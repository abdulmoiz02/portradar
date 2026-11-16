//! Drawing the dashboard.
//!
//! Colours and symbols come from `app.look` (theme, ASCII mode), so nothing
//! here hard-codes a colour or a glyph.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Paragraph, Row as TableRow, Table, Tabs, Wrap};

use crate::app::{App, KillRequest, Mode, PaletteState, TOUR_STEPS, Tab};
use crate::browser;
use crate::inspect::{human_duration, short_duration};
use crate::model::{EventKind, Group, RowState};
use crate::palette;
use crate::services::{Access, service_name};
use crate::theme::Look;
use crate::verdict::{Level, Verdict, exposure_note, verdict};

pub fn render(frame: &mut Frame, app: &mut App) {
    let now = Instant::now();
    let area = frame.area();
    let look = app.look;

    // Short or narrow terminals drop the optional parts: first the summary
    // cards, then the details panel.
    let show_cards = area.height >= 24 && area.width >= 60;
    let details_height = match (app.tab, area.height) {
        (Tab::Events, _) => 0,
        (_, h) if h >= 34 => 8,
        (_, h) if h >= 24 => 6,
        _ => 0,
    };
    let [title, cards, tabs, body, details, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if show_cards { 4 } else { 0 }),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(details_height),
        Constraint::Length(1),
    ])
    .areas(area);

    render_title_bar(frame, app, &look, title, !show_cards);
    if show_cards {
        render_cards(frame, app, &look, cards);
    }
    render_tabs(frame, app, &look, tabs);
    app.hit.rows = Rect::default(); // only the table tabs set this
    match app.tab {
        Tab::Events => render_events(frame, app, &look, body),
        _ => render_table(frame, app, &look, body, now),
    }
    if details_height > 0 {
        render_details(frame, app, &look, details);
    }
    render_footer(frame, app, &look, footer, now);

    match &app.mode {
        Mode::ConfirmKill(request) => render_kill_popup(frame, &look, request, area),
        Mode::Help => render_help(frame, &look, area),
        Mode::Palette(state) => render_palette(frame, &look, state, area),
        Mode::Tour(step) => render_tour(frame, &look, *step, area),
        _ => {}
    }
}

fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

// ---- header ------------------------------------------------------------

fn render_title_bar(frame: &mut Frame, app: &App, look: &Look, area: Rect, show_hidden_warning: bool) {
    let t = &look.theme;
    let g = &look.glyphs;
    let bar = Style::new().bg(t.bar_bg).fg(t.bar_fg);

    let mut left = vec![Span::styled(
        format!(" {} portwatch ", g.logo),
        Style::new().bg(t.accent).fg(t.on_accent).bold(),
    )];
    if area.width >= 90 {
        left.push(Span::styled("  ports on this computer, live", bar.fg(t.subtle)));
    }

    let mut right: Vec<Span> = Vec::new();
    if show_hidden_warning && !app.is_root && app.tracker.hidden_count() > 0 {
        right.push(Span::styled(
            format!(" {} {} hidden (needs sudo) ", g.warn, app.tracker.hidden_count()),
            bar.fg(t.warn),
        ));
    }
    if let Some(err) = &app.scan_error {
        right.push(Span::styled(format!(" {} scan failed: {err} ", g.warn), bar.fg(t.warn)));
    } else if app.paused {
        right.push(Span::styled(format!(" {} PAUSED ", g.paused), bar.fg(t.warn).bold()));
    } else {
        right.push(Span::styled(format!(" {} live ", g.live), bar.fg(t.good)));
    }

    frame.render_widget(ratatui::widgets::Block::new().style(bar), area);
    frame.render_widget(Paragraph::new(Line::from(left)).style(bar), area);
    frame.render_widget(
        Paragraph::new(Line::from(right))
            .style(bar)
            .alignment(Alignment::Right),
        area,
    );
}

fn render_cards(frame: &mut Frame, app: &App, look: &Look, area: Rect) {
    let t = &look.theme;
    let tracker = &app.tracker;
    let exposed = tracker.exposed_count();
    let hidden = tracker.hidden_count();
    let hidden_bad = hidden > 0 && !app.is_root;

    let cards = [
        ("Open ports", tracker.listening_count(), "listening now", t.accent),
        (
            "Exposed",
            exposed,
            if exposed == 0 { "all local only" } else { "open to network" },
            if exposed > 0 { t.warn } else { t.good },
        ),
        ("Connections", tracker.connection_count(), "active now", t.accent),
        (
            "Hidden",
            hidden,
            if hidden_bad { "need sudo to see" } else { "all owners visible" },
            if hidden_bad { t.warn } else { t.good },
        ),
    ];

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(area);
    for ((title, number, label, color), rect) in cards.into_iter().zip(columns.iter()) {
        let block = look
            .block()
            .border_style(fg(color))
            .title(Line::from(format!(" {title} ")).style(fg(color).bold()));
        let text = vec![
            Line::styled(number.to_string(), fg(color).bold()),
            Line::styled(label, fg(t.dim)),
        ];
        frame.render_widget(
            Paragraph::new(text).alignment(Alignment::Center).block(block),
            *rect,
        );
    }
}

fn render_tabs(frame: &mut Frame, app: &mut App, look: &Look, area: Rect) {
    let t = &look.theme;
    let titles: Vec<String> = Tab::ALL
        .iter()
        .map(|tab| match tab {
            Tab::Listening => format!("1 Listening ({})", app.tracker.listening_count()),
            Tab::Connections => format!("2 Connections ({})", app.tracker.connection_count()),
            Tab::Events => format!("3 Events ({})", app.tracker.events.len()),
        })
        .collect();

    // Remember where each tab label sits, for mouse clicks. Each label is
    // padded with one space on both sides and separated by a one-space divider.
    app.hit.tabs.clear();
    let mut x = area.x;
    for (tab, title) in Tab::ALL.iter().zip(&titles) {
        let width = title.chars().count() as u16 + 2;
        let right = area.x + area.width;
        if x < right {
            app.hit.tabs.push((*tab, Rect::new(x, area.y, width.min(right - x), 1)));
        }
        x = x.saturating_add(width + 1);
    }

    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .style(fg(t.dim))
        .highlight_style(Style::new().bg(t.accent).fg(t.on_accent).bold())
        .padding(" ", " ")
        .divider(" ");
    frame.render_widget(tabs, area);
}

// ---- table ---------------------------------------------------------------

fn render_table(frame: &mut Frame, app: &mut App, look: &Look, area: Rect, now: Instant) {
    let t = &look.theme;
    let g = &look.glyphs;
    let listening = app.tab == Tab::Listening;

    let groups = app.visible();
    let total = groups.len();
    let selected_index = groups
        .iter()
        .position(|grp| Some(grp.key()) == app.selected);

    let table_rows: Vec<TableRow> = groups
        .iter()
        .map(|grp| {
            let cells = if listening {
                listening_cells(grp, &app.me, look)
            } else {
                connection_cells(grp, look)
            };
            let style = match grp.state(now) {
                RowState::New => fg(t.good).bold(),
                RowState::Closed => fg(t.bad).add_modifier(Modifier::CROSSED_OUT),
                RowState::Normal => Style::new(),
            };
            TableRow::new(cells).style(style)
        })
        .collect();

    let (header, widths): (Vec<&str>, Vec<Constraint>) = if listening {
        (
            vec!["", "PORT", "SERVICE", "ACCESS", "PROCESS", "USER", "UP"],
            vec![
                Constraint::Length(1),
                Constraint::Length(9),
                Constraint::Length(15),
                Constraint::Length(15),
                Constraint::Min(14),
                Constraint::Min(8),
                Constraint::Length(5),
            ],
        )
    } else {
        (
            vec!["PROTO", "LOCAL", "REMOTE", "STATE", "PROCESS", "USER"],
            vec![
                Constraint::Length(5),
                Constraint::Min(16),
                Constraint::Min(18),
                Constraint::Length(11),
                Constraint::Min(12),
                Constraint::Min(6),
            ],
        )
    };

    let mut title = vec![
        Span::styled(
            if listening { " Listening ports " } else { " Connections " },
            fg(t.accent).bold(),
        ),
        Span::styled(
            format!(
                "{} sorted by {} {} ",
                g.dot,
                app.sort.label(),
                if app.reverse { g.down } else { g.up }
            ),
            fg(t.dim),
        ),
    ];
    for name in app.active_filters() {
        title.push(Span::styled(format!("{} {name} ", g.dot), fg(t.warn)));
    }
    if !app.filter.is_empty() {
        title.push(Span::styled(
            format!("{} search: {} ", g.dot, app.filter),
            fg(t.warn),
        ));
    }

    let table = Table::new(table_rows, widths)
        .header(TableRow::new(header).style(fg(t.dim).bold()))
        .block(look.block().border_style(fg(t.dim)).title(Line::from(title)))
        .row_highlight_style(Style::new().bg(t.select_bg).fg(t.select_fg).bold())
        .highlight_symbol(g.row_bar)
        .column_spacing(if area.width >= 100 { 2 } else { 1 });

    // `groups` borrows `app`, but everything the table needs was copied into
    // owned cells above, so the borrow can end before we touch the state.
    drop(groups);
    app.table_state.select(selected_index);
    frame.render_stateful_widget(table, area, &mut app.table_state);

    // Where the data rows ended up (inside the border, below the header),
    // so clicks can be mapped back to rows.
    let inner = Rect::new(
        area.x + 1,
        area.y + 2, // top border + header row
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    app.hit.rows = if area.width >= 2 && area.height >= 3 { inner } else { Rect::default() };
    app.hit.row_offset = app.table_state.offset();

    if total == 0 {
        let filters = app.active_filters();
        let message = if app.filter.is_empty() && filters.is_empty() {
            "Nothing here right now.".to_string()
        } else {
            let mut on: Vec<String> = filters.iter().map(|f| f.to_string()).collect();
            if !app.filter.is_empty() {
                on.push(format!("search {}{}{}", g.quote_open, app.filter, g.quote_close));
            }
            format!(
                "Nothing matches ({}). Esc clears the search; press the filter key again to turn a filter off.",
                on.join(", ")
            )
        };
        let inner = Rect::new(
            area.x + 1,
            area.y + area.height / 2,
            area.width.saturating_sub(2),
            1.min(area.height),
        );
        frame.render_widget(
            Paragraph::new(message)
                .style(fg(t.dim))
                .alignment(Alignment::Center),
            inner,
        );
    }
}

fn process_cell(grp: &Group<'_>, look: &Look) -> Cell<'static> {
    match grp.process() {
        Some(p) => Cell::from(Line::from(vec![
            Span::styled(p.name.clone(), Style::new().bold()),
            Span::styled(format!(" {}", p.pid), fg(look.theme.dim)),
        ])),
        None => Cell::from(Span::styled("? needs sudo", fg(look.theme.dim))),
    }
}

fn marker_cell(verdict: Verdict, look: &Look) -> Cell<'static> {
    let t = &look.theme;
    let g = &look.glyphs;
    match verdict.level {
        Level::Normal => Cell::from(Span::styled(g.normal, fg(t.dim))),
        Level::Dev => Cell::from(Span::styled(g.dev, fg(t.accent).bold())),
        Level::Check => Cell::from(Span::styled(g.warn, fg(t.warn).bold())),
        Level::Unknown => Cell::from(""),
    }
}

fn access_label(access: Access, look: &Look) -> (String, Color) {
    let t = &look.theme;
    let g = &look.glyphs;
    let (symbol, color) = match access {
        Access::ThisPcOnly => (g.local, t.good),
        Access::OneNetwork => (g.one, t.warn),
        Access::WholeNetwork => (g.whole, t.warn),
    };
    (format!("{symbol} {}", access.label()), color)
}

fn listening_cells(grp: &Group<'_>, me: &str, look: &Look) -> Vec<Cell<'static>> {
    let t = &look.theme;
    let port = Cell::from(Line::from(vec![
        Span::styled(grp.port().to_string(), Style::new().bold()),
        Span::styled(format!("/{}", grp.protocol()), fg(t.dim)),
    ]));
    let service = match service_name(grp.protocol(), grp.port()) {
        Some(name) => Cell::from(name),
        None => Cell::from(Span::styled("-", fg(t.dim))),
    };
    let (text, color) = access_label(grp.access(), look);
    let up = match grp.process().and_then(|p| p.running_for_secs()) {
        Some(secs) => Cell::from(short_duration(secs)),
        None => Cell::from(Span::styled("-", fg(t.dim))),
    };

    vec![
        marker_cell(verdict(grp, me), look),
        port,
        service,
        Cell::from(Span::styled(text, fg(color))),
        process_cell(grp, look),
        Cell::from(grp.user().to_string()),
        up,
    ]
}

fn connection_cells(grp: &Group<'_>, look: &Look) -> Vec<Cell<'static>> {
    let t = &look.theme;
    let e = &grp.first().entry;
    let state_color = match e.state.as_str() {
        "Established" => t.good,
        "SynSent" | "SynRecv" | "CloseWait" => t.warn,
        "TimeWait" | "FinWait1" | "FinWait2" | "LastAck" | "Closing" => t.dim,
        _ => Color::Reset,
    };
    vec![
        Cell::from(e.protocol.to_string()),
        Cell::from(e.local.to_string()),
        Cell::from(e.remote.to_string()),
        Cell::from(Span::styled(e.state.clone(), fg(state_color))),
        process_cell(grp, look),
        Cell::from(e.user.clone()),
    ]
}

// ---- events ----------------------------------------------------------------

fn render_events(frame: &mut Frame, app: &mut App, look: &Look, area: Rect) {
    let t = &look.theme;
    let g = &look.glyphs;
    let block = look.block().border_style(fg(t.dim)).title(Line::from(vec![
        Span::styled(" Events ", fg(t.accent).bold()),
        Span::styled(
            format!("{} ports opening and closing {} {} scroll ", g.dot, g.dot, g.keys_move),
            fg(t.dim),
        ),
    ]));
    let inner = block.inner(area);

    let total = app.tracker.events.len();
    let visible = inner.height as usize;
    app.events_scroll = app.events_scroll.min(total.saturating_sub(visible));

    let lines: Vec<Line> = app
        .tracker
        .events
        .iter()
        .rev() // newest first
        .skip(app.events_scroll)
        .take(visible)
        .map(|event| {
            let local: chrono::DateTime<chrono::Local> = event.at.into();
            let (marker, style) = match event.kind {
                EventKind::Opened => (format!("{} opened", g.up), fg(t.good)),
                EventKind::Closed => (format!("{} closed", g.down), fg(t.bad)),
            };
            Line::from(vec![
                Span::styled(local.format("%H:%M:%S  ").to_string(), fg(t.dim)),
                Span::styled(format!("{marker}  "), style),
                Span::raw(event.text.clone()),
            ])
        })
        .collect();

    let content = if lines.is_empty() {
        Paragraph::new("No changes yet. When a program starts or stops listening on a port, it shows up here.")
            .style(fg(t.dim))
            .wrap(Wrap { trim: true })
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(block, area);
    frame.render_widget(content, inner);
}

// ---- details ---------------------------------------------------------------

/// "127.0.0.1, ::1" for the title; long lists are shortened.
fn address_list(grp: &Group<'_>, look: &Look) -> String {
    let ips = grp.addresses();
    let mut shown: Vec<String> = ips.iter().take(3).map(|ip| ip.to_string()).collect();
    if ips.len() > 3 {
        shown.push(format!("+{} more", ips.len() - 3));
    }
    let _ = look;
    shown.join(", ")
}

fn level_label(level: Level) -> &'static str {
    match level {
        Level::Normal => "Looks normal",
        Level::Dev => "Dev server",
        Level::Check => "Look at this",
        Level::Unknown => "Not sure",
    }
}

fn render_details(frame: &mut Frame, app: &App, look: &Look, area: Rect) {
    let t = &look.theme;
    let g = &look.glyphs;
    let mut lines: Vec<Line> = Vec::new();
    let mut title = " Details ".to_string();

    if let Some(grp) = app.selected_group() {
        let e = &grp.first().entry;
        let port = grp.port();
        let access = grp.access();
        let listening = grp.listening();
        let exposure_color = if access.is_exposed() { t.warn } else { t.good };

        // The address is in the title because several rows can share a
        // port number (for example 127.0.0.53:53 and 127.0.0.54:53).
        let addresses = address_list(&grp, look);
        title = match grp.process() {
            Some(p) if listening => format!(" {} on port {port} {} {addresses} ", p.name, g.dot),
            None if listening => format!(" port {port} {} {addresses} ", g.dot),
            Some(p) => format!(" {} connection ", p.name),
            None => " connection ".to_string(),
        };

        if grp.is_closed() {
            lines.push(Line::styled(
                "This port just closed: the program stopped listening on it.",
                fg(t.bad),
            ));
        } else if !listening {
            let who = match grp.process() {
                Some(p) => format!("{} (PID {})", p.name, p.pid),
                None => format!("A program owned by {}", e.user),
            };
            lines.push(Line::from(vec![
                Span::styled(who, Style::new().bold()),
                Span::raw(format!(
                    " has a {} connection between {} and {}.",
                    e.state.to_lowercase(),
                    e.local,
                    e.remote
                )),
            ]));
        } else {
            let mut first = match grp.process() {
                Some(p) => vec![
                    Span::styled(p.name.clone(), Style::new().bold()),
                    Span::raw(format!(" (PID {}) is listening on port {port}. ", p.pid)),
                ],
                None => vec![
                    Span::raw(format!("Something is listening on port {port}, but its program belongs to ")),
                    Span::styled(e.user.clone(), Style::new().bold()),
                    Span::raw(" and is hidden from you. "),
                ],
            };
            first.push(Span::styled(access.sentence(e.local.ip()), fg(exposure_color)));
            if let Some(note) = exposure_note(&grp) {
                first.push(Span::styled(format!(" {note}"), fg(t.dim)));
            }
            lines.push(Line::from(first));

            let v = verdict(&grp, &app.me);
            let (glyph, color) = match v.level {
                Level::Normal => (g.normal, t.good),
                Level::Dev => (g.dev, t.accent),
                Level::Check => (g.warn, t.warn),
                Level::Unknown => ("?", t.dim),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{glyph} {}: ", level_label(v.level)), fg(color).bold()),
                Span::styled(v.reason, fg(t.subtle)),
            ]));

            if grp.process().is_none() {
                lines.push(Line::from(vec![
                    Span::raw("To see and manage it, run: "),
                    Span::styled("sudo \"$(which portwatch)\"", fg(t.accent).bold()),
                ]));
            }
        }

        if let (Some(p), Some(d)) = (grp.process(), &app.details) {
            let mut facts: Vec<String> = Vec::new();
            if listening && let Some(name) = service_name(grp.protocol(), port) {
                facts.push(format!("Port {port} is usually {name}"));
            }
            if let Some(secs) = d.running_for_secs.or_else(|| p.running_for_secs()) {
                facts.push(format!("started {} ago by {}", human_duration(secs), e.user));
            }
            if listening {
                let n = app.connections_on_port(port);
                facts.push(format!("{n} open connection{}", if n == 1 { "" } else { "s" }));
            }
            if !facts.is_empty() {
                lines.push(Line::styled(facts.join(&format!(" {} ", g.dot)), fg(t.subtle)));
            }
            if let Some(hint) = &d.hint {
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", g.warn), fg(t.warn).bold()),
                    Span::styled(hint.advice(), fg(t.warn)),
                ]));
            }
            if !d.cmdline.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("$ ", fg(t.accent).bold()),
                    Span::raw(d.cmdline.clone()),
                ]));
            }
            if let Some(cwd) = &d.cwd {
                lines.push(Line::styled(format!("in {cwd}"), fg(t.dim)));
            }
        }
    } else {
        lines.push(Line::styled(
            format!("Select a row with {} to see what it is.", g.keys_move),
            fg(t.dim),
        ));
    }

    let block = look
        .block()
        .border_style(fg(t.dim))
        .title(Line::styled(title, fg(t.accent).bold()));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn label_line(label: &str, value: &str, look: &Look) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), fg(look.theme.accent).bold()),
        Span::raw(value.to_string()),
    ])
}

// ---- footer and popups -------------------------------------------------------

fn key_chip(key: &str, look: &Look) -> Span<'static> {
    let t = &look.theme;
    Span::styled(
        format!(" {key} "),
        Style::new().bg(t.chip_bg).fg(t.chip_fg).bold(),
    )
}

fn active_key_chip(key: &str, look: &Look) -> Span<'static> {
    let t = &look.theme;
    Span::styled(
        format!(" {key} "),
        Style::new().bg(t.accent).fg(t.on_accent).bold(),
    )
}

/// The keys worth showing right now, most important first. Keys that cannot
/// do anything in the current tab or on the current row are left out, and
/// a filter that is switched on is drawn highlighted.
fn footer_keys(app: &App, look: &Look) -> Vec<(String, &'static str, bool)> {
    let g = &look.glyphs;
    let mv = g.keys_move.to_string();
    let tabs = g.keys_tabs.to_string();
    match app.tab {
        Tab::Events => vec![
            (mv, "Scroll", false),
            ("?".into(), "Help", false),
            ("q".into(), "Quit", false),
            (":".into(), "Menu", false),
            (tabs, "Tabs", false),
        ],
        Tab::Connections => vec![
            (mv, "Move", false),
            ("/".into(), "Search", false),
            ("?".into(), "Help", false),
            ("q".into(), "Quit", false),
            (":".into(), "Menu", false),
            (tabs, "Tabs", false),
            ("m".into(), "Mine", app.only_mine),
            ("s".into(), "Sort", false),
            ("p".into(), "Pause", app.paused),
        ],
        Tab::Listening => {
            let can_open = app
                .selected_group()
                .and_then(|grp| browser::url_for(&grp))
                .is_some();
            let mut keys = vec![
                (mv, "Move", false),
                ("/".into(), "Search", false),
                ("?".into(), "Help", false),
                ("q".into(), "Quit", false),
                (":".into(), "Menu", false),
                ("k".into(), "Stop", false),
                ("f".into(), "Free", false),
            ];
            if can_open {
                keys.push(("o".into(), "Open", false));
            }
            keys.push(("e".into(), "Exposed", app.only_exposed));
            keys.push(("m".into(), "Mine", app.only_mine));
            keys.push(("h".into(), "Quiet", app.hide_noise));
            keys.push((tabs, "Tabs", false));
            keys.push(("s".into(), "Sort", false));
            keys.push(("p".into(), "Pause", app.paused));
            keys
        }
    }
}

fn render_footer(frame: &mut Frame, app: &App, look: &Look, area: Rect, now: Instant) {
    let t = &look.theme;
    let line = if matches!(app.mode, Mode::Filter) {
        Line::from(vec![
            Span::styled(" Search ", Style::new().bg(t.warn).fg(t.on_accent).bold()),
            Span::raw(" "),
            Span::raw(app.filter.clone()),
            Span::styled(look.glyphs.cursor, fg(t.warn)),
            Span::styled("   Enter keep · Esc clear", fg(t.dim)),
        ])
    } else if let Some(status) = app.status_text(now) {
        let color = if status.is_error { t.bad } else { t.good };
        Line::from(Span::styled(format!(" {} ", status.text), fg(color).bold()))
    } else {
        let mut spans: Vec<Span> = Vec::new();
        let mut width = 0usize;
        for (key, what, active) in footer_keys(app, look) {
            let needed = key.chars().count() + 2 + what.chars().count() + 3;
            if width + needed > area.width as usize {
                break;
            }
            width += needed;
            spans.push(if active {
                active_key_chip(&key, look)
            } else {
                key_chip(&key, look)
            });
            spans.push(Span::styled(format!(" {what}  "), fg(t.subtle)));
        }
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn render_kill_popup(frame: &mut Frame, look: &Look, request: &KillRequest, area: Rect) {
    let t = &look.theme;
    let g = &look.glyphs;
    let signal = if request.force {
        "SIGKILL: stops it immediately, it cannot save anything"
    } else {
        "SIGTERM: asks it to shut down cleanly"
    };
    let question = if request.free {
        vec![
            Span::raw(format!("Port {} is used by ", request.port)),
            Span::styled(request.name.clone(), Style::new().bold()),
            Span::raw(format!(" (PID {}). Stop it to free the port?", request.pid)),
        ]
    } else {
        vec![
            Span::raw("Stop "),
            Span::styled(request.name.clone(), Style::new().bold()),
            Span::raw(format!(" (PID {}), which is using port {}?", request.pid, request.port)),
        ]
    };
    let mut lines = vec![
        Line::from(question),
        Line::from(""),
        label_line("How", signal, look),
    ];
    if let Some(advice) = &request.advice {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", g.warn), fg(t.warn).bold()),
            Span::styled(advice.clone(), fg(t.warn)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        key_chip("y", look),
        Span::raw(" Yes, stop it     "),
        key_chip("any other key", look),
        Span::raw(" Cancel"),
    ]));

    let height = lines.len() as u16 + 4;
    let popup = centered(area, 68, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            look.block().border_style(fg(t.bad)).title(Line::styled(
                if request.free { " Free this port " } else { " Confirm " },
                fg(t.bad).bold(),
            )),
        ),
        popup,
    );
}

fn render_help(frame: &mut Frame, look: &Look, area: Rect) {
    let t = &look.theme;
    let g = &look.glyphs;
    let move_keys = format!("{}  PgUp PgDn  g G", g.keys_move);
    let tab_keys = format!("{}  Tab  1 2 3", g.keys_tabs);
    let keys: Vec<(String, &str)> = vec![
        (move_keys, "move the selection"),
        (tab_keys, "switch tab"),
        ("/".into(), "search by port, service, program, user..."),
        ("Esc".into(), "clear the search"),
        ("k  /  K".into(), "stop the program (polite / force), asks first"),
        ("f".into(), "free the port: stop its program, then check it is free"),
        ("o".into(), "open the port in your web browser (TCP only)"),
        ("e  m  h".into(), "show only exposed / only mine / hide system noise"),
        ("s  /  r".into(), "change the sort column / reverse it"),
        ("p".into(), "pause or resume refreshing"),
        (":  or  Ctrl+P".into(), "command menu: find any action by name"),
        ("q  /  Ctrl+C".into(), "quit"),
        ("Mouse".into(), "click tabs and rows, wheel scrolls (hold Shift to select text)"),
    ];
    let mut lines = vec![
        Line::styled("Green rows just opened. Red rows just closed.", fg(t.dim)),
        Line::from(""),
    ];
    for (key, what) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<22}"), fg(t.accent).bold()),
            Span::raw(what),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("What the columns and marks mean", Style::new().bold()));
    lines.push(Line::from(vec![
        Span::styled("SERVICE  ", fg(t.accent).bold()),
        Span::raw("what that port number is normally used for (a guess)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("ACCESS   ", fg(t.accent).bold()),
        Span::styled(format!("{} this PC only", g.local), fg(t.good)),
        Span::raw(": nothing else can connect  "),
        Span::styled(format!("{} whole network", g.whole), fg(t.warn)),
        Span::raw(": other devices can try"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("MARK     ", fg(t.accent).bold()),
        Span::styled(format!("{} ", g.normal), fg(t.dim)),
        Span::raw("normal   "),
        Span::styled(format!("{} ", g.dev), fg(t.accent)),
        Span::raw("your dev server   "),
        Span::styled(format!("{} ", g.warn), fg(t.warn)),
        Span::raw("worth a look (a hint, not an audit)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("UP       ", fg(t.accent).bold()),
        Span::raw("how long the program has been running"),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::styled("Press any key to close", fg(t.dim)));

    let popup = centered(area, 88, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            look.block()
                .border_style(fg(t.accent))
                .title(Line::styled(" Help ", fg(t.accent).bold())),
        ),
        popup,
    );
}

fn render_palette(frame: &mut Frame, look: &Look, state: &PaletteState, area: Rect) {
    let t = &look.theme;
    let g = &look.glyphs;
    let results = palette::matching(&state.input);
    let shown = results.len().min(10);
    // Scroll the window so the highlighted result is always visible.
    let start = if state.selected >= shown { state.selected + 1 - shown } else { 0 };
    let width = 66u16.min(area.width);
    let inner = width.saturating_sub(2) as usize;

    let mut lines = vec![
        Line::from(vec![
            Span::styled(": ", fg(t.accent).bold()),
            Span::raw(state.input.clone()),
            Span::styled(g.cursor, fg(t.warn)),
        ]),
        Line::from(""),
    ];
    if results.is_empty() {
        lines.push(Line::styled("No matching command. Try a different word.", fg(t.dim)));
    }
    for (i, entry) in results.iter().enumerate().skip(start).take(shown) {
        let selected = i == state.selected;
        let marker = if selected { g.row_bar } else { "  " };
        let key_width = 5usize;
        let name_width = inner.saturating_sub(marker.chars().count() + key_width);
        let name: String = entry.name.chars().take(name_width).collect();
        let text = format!("{marker}{name:<name_width$}{:>key_width$}", entry.key);
        lines.push(Line::styled(
            text,
            if selected {
                Style::new().bg(t.select_bg).fg(t.select_fg).bold()
            } else {
                Style::new()
            },
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        format!("Enter run {} Esc close {} {} choose", g.dot, g.dot, g.keys_move),
        fg(t.dim),
    ));

    let height = lines.len() as u16 + 2;
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            look.block()
                .border_style(fg(t.accent))
                .title(Line::styled(" Command menu ", fg(t.accent).bold())),
        ),
        popup,
    );
}

fn render_tour(frame: &mut Frame, look: &Look, step: usize, area: Rect) {
    let t = &look.theme;
    let g = &look.glyphs;
    let key = |k: &str, what: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{k:<10}"), fg(t.accent).bold()),
            Span::raw(what.to_string()),
        ])
    };

    let (title, mut lines): (&str, Vec<Line>) = match step {
        0 => (
            " Welcome to portwatch ",
            vec![
                Line::from("Think of your computer as a building."),
                Line::from(""),
                Line::from("A port is a numbered door. A program opens a door to wait for"),
                Line::from("visitors: this is called listening."),
                Line::from(""),
                Line::from("portwatch shows every open door and who opened it, live."),
                Line::from("It can also close a door by stopping the program (always asking first)."),
            ],
        ),
        1 => (
            " How to read a row ",
            vec![
                key("PORT", "the door number, and tcp or udp"),
                key("SERVICE", "what that number is usually used for (a guess)"),
                key("ACCESS", "who can knock:"),
                Line::from(vec![
                    Span::raw("          "),
                    Span::styled(format!("{} this PC only", g.local), fg(t.good)),
                    Span::raw(": nobody else can connect"),
                ]),
                Line::from(vec![
                    Span::raw("          "),
                    Span::styled(format!("{} whole network", g.whole), fg(t.warn)),
                    Span::raw(": other devices can try"),
                ]),
                key("PROCESS", "the program that opened it, and its PID"),
                key("UP", "how long it has been open"),
                Line::from(vec![
                    Span::styled(format!("{:<10}", "MARK"), fg(t.accent).bold()),
                    Span::styled(format!("{} ", g.normal), fg(t.dim)),
                    Span::raw("normal   "),
                    Span::styled(format!("{} ", g.dev), fg(t.accent)),
                    Span::raw("your dev server   "),
                    Span::styled(format!("{} ", g.warn), fg(t.warn)),
                    Span::raw("worth a look"),
                ]),
            ],
        ),
        _ => (
            " Keys to know ",
            vec![
                key(g.keys_move, "move up and down"),
                key("/", "search (try: mysql, network, or a port number)"),
                key("k", "stop the selected program (asks first)"),
                key("f", "free a port: stop its program and check it is free"),
                key("o", "open a port in your web browser"),
                key(":", "command menu: find any action by name"),
                key("?", "all keys, and what the columns mean"),
                key("q", "quit"),
            ],
        ),
    };
    lines.push(Line::from(""));
    lines.push(Line::styled(
        format!("Step {} of {TOUR_STEPS}   Enter: next   Left: back   Esc: skip", step + 1),
        fg(t.dim),
    ));

    let popup = centered(area, 76, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            look.block()
                .border_style(fg(t.accent))
                .title(Line::styled(title, fg(t.accent).bold())),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{PortEntry, ProcessInfo, Protocol};
    use crate::theme::{Look, ThemeName};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::SystemTime;

    fn entry(port: u16, inode: u64, name: &str, pid: u32) -> PortEntry {
        PortEntry {
            protocol: Protocol::Tcp,
            local: format!("127.0.0.1:{port}").parse().unwrap(),
            remote: "0.0.0.0:0".parse().unwrap(),
            state: "Listen".into(),
            listening: true,
            inode,
            user: "alice".into(),
            process: Some(ProcessInfo {
                pid,
                name: name.into(),
                started_at: None,
            }),
        }
    }

    fn at(mut e: PortEntry, ip: &str) -> PortEntry {
        let port = e.local.port();
        e.local = if ip.contains(':') {
            format!("[{ip}]:{port}").parse().unwrap()
        } else {
            format!("{ip}:{port}").parse().unwrap()
        };
        e
    }

    fn draw(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn new_app(root: bool) -> App {
        App::new(root, "alice".into())
    }

    fn select_first(app: &mut App) {
        app.selected = app.visible().first().map(|g| g.key());
    }

    fn app() -> App {
        let mut app = new_app(true);
        app.tracker.update(
            vec![
                entry(3000, 1, "node", 900_001),
                entry(5432, 2, "postgres", 900_002),
            ],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app);
        app
    }

    /// A non-root app with a visible owner, a hidden owner exposed to the
    /// whole network, and one established connection: what a normal user
    /// sees on a normal machine.
    fn app_with_hidden_owner() -> App {
        let mut app = new_app(false);
        let mut hidden = at(entry(631, 3, "x", 1), "0.0.0.0");
        hidden.process = None;
        let mut conn = entry(3000, 4, "node", 900_001);
        conn.listening = false;
        conn.state = "Established".into();
        conn.remote = "10.20.30.40:55555".parse().unwrap();
        app.tracker.update(
            vec![entry(3000, 1, "node", 900_001), hidden, conn],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app);
        app
    }

    #[test]
    fn renders_cards_tabs_table_and_footer() {
        let mut app = app();
        let screen = draw(&mut app, 110, 30);
        for expected in [
            "portwatch",
            "Open ports",
            "Exposed",
            "Connections",
            "Hidden",
            "Listening (2)",
            "PORT",
            "SERVICE",
            "ACCESS",
            "3000/tcp",
            "Dev server",
            "PostgreSQL",
            "this PC only",
            "postgres",
            "Quit",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
    }

    #[test]
    fn details_explain_the_selected_port_in_plain_words() {
        let mut app = app();
        app.details = Some(crate::inspect::Details {
            cmdline: "node server.js".into(),
            cwd: Some("/home/alice/app".into()),
            running_for_secs: Some(7_500),
            hint: None,
        });
        let screen = draw(&mut app, 110, 40);
        for expected in [
            "node on port 3000",
            "is listening on port 3000",
            "Only this computer can reach it",
            "Dev server: A development server you started",
            "started 2h 5m ago by alice",
            "$ node server.js",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
    }

    #[test]
    fn rows_of_one_program_on_ipv4_and_ipv6_are_shown_once() {
        let mut app = new_app(false);
        app.tracker.update(
            vec![
                entry(6379, 1, "redis-server", 700),
                at(entry(6379, 2, "redis-server", 700), "::1"),
            ],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app);
        let screen = draw(&mut app, 110, 30);
        assert_eq!(screen.matches("6379/tcp").count(), 1, "{screen}");
        assert!(screen.contains("Listening (1)"), "{screen}");
        // The title lists both addresses.
        assert!(screen.contains("127.0.0.1, ::1"), "{screen}");
    }

    #[test]
    fn verdict_marks_appear_in_the_first_column() {
        let mut app = new_app(false);
        let exposed_dev = at(entry(8000, 2, "python3", 5), "0.0.0.0");
        app.tracker.update(
            vec![entry(3000, 1, "node", 4), exposed_dev, entry(5432, 3, "postgres", 6)],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app);
        let screen = draw(&mut app, 110, 30);
        let line_of = |needle: &str| screen.lines().find(|l| l.contains(needle)).unwrap().to_string();
        assert!(line_of("3000/tcp").contains('★'), "dev server: {}", line_of("3000/tcp"));
        assert!(line_of("8000/tcp").contains('⚠'), "exposed dev server: {}", line_of("8000/tcp"));
        assert!(line_of("5432/tcp").contains('✓'), "local database: {}", line_of("5432/tcp"));
    }

    #[test]
    fn exposed_ports_are_called_out() {
        let mut app = app_with_hidden_owner();
        let screen = draw(&mut app, 110, 30);
        assert!(screen.contains("whole network"), "{screen}");
        assert!(screen.contains("needs sudo"), "{screen}");
        assert!(screen.contains("need sudo to see"), "hidden card:\n{screen}");
    }

    #[test]
    fn layout_holds_up_on_narrow_terminals() {
        let mut app = app_with_hidden_owner();
        for width in [80u16, 100, 140] {
            let screen = draw(&mut app, width, 26);
            for expected in [
                "3 Events",
                "Hidden",
                "SERVICE",
                "ACCESS",
                "UP",
                "3000/tcp",
                "open to network",
                "Help",
                "Quit",
            ] {
                assert!(screen.contains(expected), "missing {expected:?} at {width} cols:\n{screen}");
            }
        }
    }

    #[test]
    fn footer_shows_context_keys_and_always_keeps_help_and_quit() {
        let mut app = app();
        // Wide: everything for the Listening tab, including Open for a TCP row.
        let wide = draw(&mut app, 140, 26);
        for expected in ["Stop", "Free", "Open", "Exposed", "Mine", "Quiet", "Quit"] {
            assert!(wide.contains(expected), "missing {expected:?}:\n{wide}");
        }
        // Narrow: lower-priority keys drop off, Help and Quit stay.
        let narrow = draw(&mut app, 60, 26);
        let footer = narrow.lines().last().unwrap().to_string();
        assert!(footer.contains("Help") && footer.contains("Quit"), "{footer}");
        assert!(!footer.contains("Pause"), "low-priority keys should drop off: {footer}");
        // Events tab has no row actions.
        app.tab = Tab::Events;
        let events = draw(&mut app, 140, 26);
        let footer = events.lines().last().unwrap().to_string();
        assert!(!footer.contains("Stop") && footer.contains("Scroll"), "{footer}");
    }

    #[test]
    fn active_filters_are_named_in_the_table_title() {
        let mut app = app();
        app.only_mine = true;
        app.only_exposed = true;
        let screen = draw(&mut app, 120, 26);
        assert!(screen.contains("mine only"), "{screen}");
        assert!(screen.contains("exposed only"), "{screen}");
        // Nothing is both exposed and in this list, so the table explains why.
        assert!(screen.contains("Nothing matches"), "{screen}");
    }

    #[test]
    fn connections_tab_keeps_state_readable_at_80_columns() {
        let mut app = app_with_hidden_owner();
        app.tab = Tab::Connections;
        app.selected = None;
        let screen = draw(&mut app, 80, 26);
        assert!(screen.contains("Established"), "state squeezed:\n{screen}");
        assert!(screen.contains("10.20.30.40:55555"), "remote cut off:\n{screen}");
    }

    #[test]
    fn short_terminals_drop_cards_and_details_but_keep_rows_and_warning() {
        let mut app = app_with_hidden_owner();
        let screen = draw(&mut app, 100, 14);
        assert!(!screen.contains("Open ports"));
        assert!(!screen.contains("Details"));
        assert!(screen.contains("3000/tcp"));
        assert!(screen.contains("1 hidden"), "warning moves to the title bar:\n{screen}");
    }

    #[test]
    fn empty_search_explains_itself() {
        let mut app = app();
        app.filter = "zzz".into();
        let screen = draw(&mut app, 100, 26);
        assert!(screen.contains("Nothing matches"), "{screen}");
    }

    #[test]
    fn ascii_mode_draws_nothing_but_ascii() {
        let mut app = app_with_hidden_owner().with_look(Look::new(ThemeName::Default, true));
        select_first(&mut app);
        for (w, h) in [(110, 36), (80, 24), (100, 14)] {
            let screen = draw(&mut app, w, h);
            let bad: Vec<char> = screen.chars().filter(|c| !c.is_ascii()).collect();
            assert!(bad.is_empty(), "non-ASCII {bad:?} at {w}x{h}:\n{screen}");
        }
        // The popups too.
        app.mode = Mode::Help;
        let screen = draw(&mut app, 110, 40);
        assert!(screen.is_ascii(), "help is not ASCII:\n{screen}");
        app.mode = Mode::ConfirmKill(KillRequest {
            pid: 1234,
            name: "node".into(),
            protocol: Protocol::Tcp,
            port: 3000,
            force: false,
            free: true,
            advice: Some("Managed by systemd (x.service)".into()),
        });
        let screen = draw(&mut app, 110, 30);
        assert!(screen.is_ascii(), "popup is not ASCII:\n{screen}");
        assert!(screen.contains("Free this port"));
    }

    #[test]
    fn the_command_menu_shows_typed_text_and_matching_commands() {
        let mut app = app();
        app.mode = Mode::Palette(PaletteState { input: "sort".into(), selected: 1 });
        let screen = draw(&mut app, 100, 30);
        for expected in ["Command menu", ": sort", "Sort by port", "Sort by uptime", "Enter run"] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
        assert!(!screen.contains("Quit portwatch"), "filtered out:\n{screen}");

        app.mode = Mode::Palette(PaletteState { input: "zzzz".into(), selected: 0 });
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("No matching command"), "{screen}");

        // A long result list keeps the highlighted entry on screen.
        app.mode = Mode::Palette(PaletteState { input: String::new(), selected: 20 });
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Quit portwatch") || screen.contains("welcome tour"), "{screen}");
    }

    #[test]
    fn the_tour_has_three_readable_pages() {
        let mut app = app();
        let expectations = [
            (0, ["Welcome to portwatch", "numbered door", "Step 1 of 3"]),
            (1, ["How to read a row", "whole network", "Step 2 of 3"]),
            (2, ["Keys to know", "command menu", "Step 3 of 3"]),
        ];
        for (step, expected) in expectations {
            app.mode = Mode::Tour(step);
            let screen = draw(&mut app, 100, 30);
            for text in expected {
                assert!(screen.contains(text), "page {step} missing {text:?}:\n{screen}");
            }
        }
        // And on a small terminal it must not panic.
        let _ = draw(&mut app, 40, 10);
    }

    #[test]
    fn ascii_mode_also_covers_the_menu_and_the_tour() {
        let mut app = app().with_look(Look::new(ThemeName::Default, true));
        select_first(&mut app);
        for mode in [
            Mode::Palette(PaletteState::default()),
            Mode::Tour(0),
            Mode::Tour(1),
            Mode::Tour(2),
        ] {
            app.mode = mode;
            let screen = draw(&mut app, 100, 36);
            assert!(screen.is_ascii(), "not ASCII in {:?}:\n{screen}", app.mode);
        }
    }

    #[test]
    fn mouse_clicks_land_on_what_was_drawn() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let click = |app: &mut App, x: u16, y: u16| {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            });
        };

        let mut app = app();
        let _ = draw(&mut app, 100, 30);
        assert_eq!(app.hit.tabs.len(), 3);

        // The second data row is the second port.
        let rows = app.hit.rows;
        assert!(rows.height > 2, "{rows:?}");
        click(&mut app, rows.x + 3, rows.y + 1);
        assert_eq!(app.selected_group().unwrap().port(), 5432);
        click(&mut app, rows.x + 3, rows.y);
        assert_eq!(app.selected_group().unwrap().port(), 3000);

        // The text "2 Connections" is drawn exactly where the hit map says.
        let (tab, rect) = app.hit.tabs[1];
        assert_eq!(tab, Tab::Connections);
        let screen = draw(&mut app, 100, 30);
        let tab_row = screen.lines().nth(rect.y as usize).unwrap();
        let drawn: String = tab_row.chars().skip(rect.x as usize).take(rect.width as usize).collect();
        assert!(drawn.contains("2 Connections"), "hit rect {rect:?} covers {drawn:?}");
        click(&mut app, rect.x + 2, rect.y);
        assert_eq!(app.tab, Tab::Connections);
    }

    #[test]
    fn every_theme_renders() {
        for theme in [ThemeName::Default, ThemeName::Light, ThemeName::Mono] {
            let mut app = app_with_hidden_owner().with_look(Look::new(theme, false));
            select_first(&mut app);
            let screen = draw(&mut app, 100, 30);
            assert!(screen.contains("3000/tcp"));
        }
    }

    #[test]
    fn renders_kill_popup_and_help() {
        let mut app = app();
        app.mode = Mode::ConfirmKill(KillRequest {
            pid: 900_001,
            name: "node".into(),
            protocol: Protocol::Tcp,
            port: 3000,
            force: false,
            free: false,
            advice: Some("Managed by systemd (x.service)".into()),
        });
        let screen = draw(&mut app, 110, 30);
        assert!(screen.contains("Confirm"));
        assert!(screen.contains("SIGTERM"));
        assert!(screen.contains("systemd"));
        assert!(screen.contains("Yes, stop it"));

        app.mode = Mode::Help;
        let screen = draw(&mut app, 110, 40);
        assert!(screen.contains("Help"));
        assert!(screen.contains("What the columns and marks mean"));
        assert!(screen.contains("free the port"));
        assert!(screen.contains("Press any key to close"));
    }

    /// Preview the layout as plain text:
    /// `cargo test print_sample_screens -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_sample_screens() {
        let mut app = app_with_hidden_owner();
        println!("--- 100x30 listening\n{}", draw(&mut app, 100, 30));
        println!("--- 80x24 listening\n{}", draw(&mut app, 80, 24));
        app.tab = Tab::Connections;
        app.selected = None;
        println!("--- 80x24 connections\n{}", draw(&mut app, 80, 24));
        app.tab = Tab::Listening;
        app.mode = Mode::Help;
        println!("--- 100x40 help\n{}", draw(&mut app, 100, 40));

        // Real data from this machine.
        let mut live = App::new(nix::unistd::geteuid().is_root(), "me".into());
        live.rescan();
        println!("--- this machine, 120x38\n{}", draw(&mut live, 120, 38));
    }

    #[test]
    fn renders_events_tab_and_survives_tiny_terminals() {
        let mut app = app();
        app.tab = Tab::Events;
        let screen = draw(&mut app, 100, 26);
        assert!(screen.contains("No changes yet"));
        // Must not panic on very small screens, in any tab or mode.
        for tab in Tab::ALL {
            app.tab = tab;
            for (w, h) in [(20, 5), (1, 1), (60, 3), (200, 60)] {
                let _ = draw(&mut app, w, h);
            }
        }
        app.mode = Mode::Help;
        let _ = draw(&mut app, 30, 8);
    }
}
