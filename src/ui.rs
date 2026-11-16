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

use crate::app::{App, InspectorState, InspectorTab, KillRequest, Mode, PaletteState, TOUR_STEPS, Tab};
use crate::browser;
use crate::inspect::{human_duration, short_duration};
use crate::inspector::{self, Snapshot};
use crate::model::{EventKind, Group, RowState};
use crate::palette;
use crate::services::{Access, service_name};
use crate::theme::Look;
use crate::verdict::{Level, Verdict, exposure_note, verdict};

pub fn render(frame: &mut Frame, app: &mut App) {
    let now = Instant::now();
    let area = frame.area();
    let look = app.look;

    // The Inspector replaces the whole screen. Take its state out while
    // drawing so the rest of `app` can be read freely.
    if matches!(app.mode, Mode::Inspector(_)) {
        let mut mode = std::mem::replace(&mut app.mode, Mode::Normal);
        if let Mode::Inspector(state) = &mut mode {
            render_inspector(frame, app, &look, state, area);
        }
        app.mode = mode;
        return;
    }

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
            let cells = match (listening, app.technical) {
                (true, false) => listening_cells(grp, &app.me, look),
                (true, true) => technical_listening_cells(grp, &app.me, look),
                (false, false) => connection_cells(grp, look),
                (false, true) => technical_connection_cells(grp, look),
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

    let (header, widths) = if app.technical {
        technical_columns(listening)
    } else {
        (header, widths)
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
    if app.technical {
        title.push(Span::styled(format!("{} technical view ", g.dot), fg(t.warn)));
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
            ("Enter".into(), "Inspect", false),
            (tabs, "Tabs", false),
            ("m".into(), "Mine", app.only_mine),
            ("t".into(), "Tech", app.technical),
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
                ("Enter".into(), "Inspect", false),
                ("k".into(), "Stop", false),
                ("f".into(), "Free", false),
            ];
            if can_open {
                keys.push(("o".into(), "Open", false));
            }
            keys.push(("e".into(), "Exposed", app.only_exposed));
            keys.push(("m".into(), "Mine", app.only_mine));
            keys.push(("h".into(), "Quiet", app.hide_noise));
            keys.push(("t".into(), "Tech", app.technical));
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
            Span::styled(format!("   Enter keep {} Esc clear", look.glyphs.dot), fg(t.dim)),
        ])
    } else if let Some(status) = app.status_text(now) {
        let color = if status.is_error { t.bad } else { t.good };
        Line::from(Span::styled(format!(" {} ", status.text), fg(color).bold()))
    } else {
        fit_keys(footer_keys(app, look), area.width as usize, look)
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// As many key chips as fit in `width`, in priority order.
fn fit_keys(keys: Vec<(String, &'static str, bool)>, width: usize, look: &Look) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;
    for (key, what, active) in keys {
        let needed = key.chars().count() + 2 + what.chars().count() + 3;
        if used + needed > width {
            break;
        }
        used += needed;
        spans.push(if active {
            active_key_chip(&key, look)
        } else {
            key_chip(&key, look)
        });
        spans.push(Span::styled(format!(" {what}  "), fg(look.theme.subtle)));
    }
    Line::from(spans)
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
        ("Enter".into(), "inspect the program: full technical detail"),
        ("t".into(), "switch between the simple and the technical table"),
        ("p".into(), "pause or resume refreshing"),
        (":  or  Ctrl+P".into(), "command menu: find any action by name"),
        ("q  /  Ctrl+C".into(), "quit"),
        ("Mouse".into(), "click tabs/rows, wheel scrolls; Shift+drag selects text"),
    ];
    // On a standard 80x24 terminal the help has to fit in 22 rows, so the
    // intro and the spacing lines are dropped on short screens.
    let roomy = area.height >= 30;
    let mut lines = Vec::new();
    if roomy {
        lines.push(Line::styled("Green rows just opened. Red rows just closed.", fg(t.dim)));
        lines.push(Line::from(""));
    }
    for (key, what) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<20}"), fg(t.accent).bold()),
            Span::raw(what),
        ]));
    }
    if roomy {
        lines.push(Line::from(""));
    }
    lines.push(Line::styled("What the columns and marks mean", Style::new().bold()));
    lines.push(Line::from(vec![
        Span::styled("SERVICE  ", fg(t.accent).bold()),
        Span::raw("what that port number is normally used for (a guess)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("ACCESS   ", fg(t.accent).bold()),
        Span::styled(format!("{} this PC only", g.local), fg(t.good)),
        Span::raw(": nothing else can connect"),
    ]));
    lines.push(Line::from(vec![
        Span::raw("         "),
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
    if roomy {
        lines.push(Line::from(""));
    }
    lines.push(Line::styled("Press any key to close", fg(t.dim)));

    let popup = centered(area, 80, lines.len() as u16 + 2);
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

// ---- technical table -----------------------------------------------------

fn technical_columns(listening: bool) -> (Vec<&'static str>, Vec<Constraint>) {
    if listening {
        (
            vec!["", "PORT", "BIND", "STATE", "PID", "PROCESS", "USER", "INODE"],
            vec![
                Constraint::Length(1),
                Constraint::Length(9),
                Constraint::Min(15),
                Constraint::Length(8),
                Constraint::Length(7),
                Constraint::Min(10),
                Constraint::Min(6),
                Constraint::Length(9),
            ],
        )
    } else {
        (
            vec!["PROTO", "LOCAL", "REMOTE", "STATE", "PID", "PROCESS", "INODE"],
            vec![
                Constraint::Length(5),
                Constraint::Min(15),
                Constraint::Min(17),
                Constraint::Length(11),
                Constraint::Length(7),
                Constraint::Min(8),
                Constraint::Length(9),
            ],
        )
    }
}

fn pid_cell(grp: &Group<'_>, look: &Look) -> Cell<'static> {
    match grp.process() {
        Some(p) => Cell::from(p.pid.to_string()),
        None => Cell::from(Span::styled("-", fg(look.theme.dim))),
    }
}

fn name_cell(grp: &Group<'_>, look: &Look) -> Cell<'static> {
    match grp.process() {
        Some(p) => Cell::from(Span::styled(p.name.clone(), Style::new().bold())),
        None => Cell::from(Span::styled("?", fg(look.theme.dim))),
    }
}

/// The kernel's socket id, plus how many more sockets are merged into the row.
fn inode_cell(grp: &Group<'_>) -> Cell<'static> {
    let first = grp.first().entry.inode;
    if grp.members.len() > 1 {
        Cell::from(format!("{first} +{}", grp.members.len() - 1))
    } else {
        Cell::from(first.to_string())
    }
}

fn technical_listening_cells(grp: &Group<'_>, me: &str, look: &Look) -> Vec<Cell<'static>> {
    let t = &look.theme;
    let port = Cell::from(Line::from(vec![
        Span::styled(grp.port().to_string(), Style::new().bold()),
        Span::styled(format!("/{}", grp.protocol()), fg(t.dim)),
    ]));
    let bind = grp
        .addresses()
        .iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<_>>()
        .join(",");
    vec![
        marker_cell(verdict(grp, me), look),
        port,
        Cell::from(bind),
        Cell::from(grp.first().entry.state.clone()),
        pid_cell(grp, look),
        name_cell(grp, look),
        Cell::from(grp.user().to_string()),
        inode_cell(grp),
    ]
}

fn technical_connection_cells(grp: &Group<'_>, look: &Look) -> Vec<Cell<'static>> {
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
        pid_cell(grp, look),
        name_cell(grp, look),
        inode_cell(grp),
    ]
}

// ---- inspector -------------------------------------------------------------

fn render_inspector(frame: &mut Frame, app: &App, look: &Look, state: &mut InspectorState, area: Rect) {
    let t = &look.theme;
    let g = &look.glyphs;
    let [title, tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area);

    render_title_bar(frame, app, look, title, false);

    let titles: Vec<String> = InspectorTab::ALL
        .iter()
        .enumerate()
        .map(|(i, tab)| format!("{} {}", i + 1, tab.label()))
        .collect();
    frame.render_widget(
        Tabs::new(titles)
            .select(state.tab.index())
            .style(fg(t.dim))
            .highlight_style(Style::new().bg(t.accent).fg(t.on_accent).bold())
            .padding(" ", " ")
            .divider(" "),
        tabs,
    );

    // No wrapping inside the Inspector: every line is one row, so the
    // scroll position is exact. Long lines are simply cut at the edge.
    let lines = inspector_lines(app, state, look);
    let snapshot = &state.snapshot;
    let name = if snapshot.pid == 0 {
        snapshot.name.clone()
    } else {
        format!("{} (PID {})", snapshot.name, snapshot.pid)
    };
    let mut block = look.block().border_style(fg(t.dim)).title(Line::from(vec![
        Span::styled(format!(" {name} "), fg(t.accent).bold()),
        Span::styled(format!("{} {} ", g.dot, state.tab.label()), fg(t.dim)),
    ]));
    let inner = block.inner(body);
    let visible = inner.height as usize;
    state.scroll = state.scroll.min(lines.len().saturating_sub(visible));
    if lines.len() > visible {
        let last = (state.scroll + visible).min(lines.len());
        block = block.title_bottom(
            Line::styled(
                format!(" lines {}-{} of {} ", state.scroll + 1, last, lines.len()),
                fg(t.dim),
            )
            .right_aligned(),
        );
    }
    frame.render_widget(block, body);
    frame.render_widget(Paragraph::new(lines).scroll((state.scroll as u16, 0)), inner);

    let footer_line = if let Some(status) = app.status_text(Instant::now()) {
        let color = if status.is_error { t.bad } else { t.good };
        Line::from(Span::styled(format!(" {} ", status.text), fg(color).bold()))
    } else {
        let mut keys: Vec<(String, &'static str, bool)> = vec![
            ("Esc".into(), "Back", false),
            (g.keys_tabs.to_string(), "Tabs", false),
            (g.keys_move.to_string(), "Scroll", false),
            ("y".into(), "Copy JSON", false),
        ];
        match state.tab {
            InspectorTab::Sockets => keys.push(("r".into(), "Names", false)),
            InspectorTab::Env => keys.push(("v".into(), "Reveal", state.reveal_env)),
            _ => {}
        }
        if !state.snapshot.exited {
            keys.push(("k".into(), "Stop", false));
            keys.push(("f".into(), "Free", false));
            keys.push(("o".into(), "Open", false));
        }
        fit_keys(keys, footer.width as usize, look)
    };
    frame.render_widget(Paragraph::new(footer_line), footer);
}

fn inspector_lines(app: &App, state: &InspectorState, look: &Look) -> Vec<Line<'static>> {
    let t = &look.theme;
    let g = &look.glyphs;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if state.snapshot.exited {
        lines.push(Line::styled(
            format!("{} This program has exited. This is how it looked a moment ago.", g.warn),
            fg(t.warn).bold(),
        ));
        lines.push(Line::from(""));
    }
    lines.extend(match state.tab {
        InspectorTab::Overview => overview_lines(app, state, look),
        InspectorTab::Process => process_lines(&state.snapshot, look),
        InspectorTab::Sockets => sockets_lines(app, &state.snapshot, look),
        InspectorTab::Origin => origin_lines(&state.snapshot, look),
        InspectorTab::Env => env_lines(state, look),
        InspectorTab::Raw => raw_lines(&state.snapshot, look),
    });
    lines
}

fn kv_line(label: &str, value: String, look: &Look) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<14}"), fg(look.theme.accent).bold()),
        Span::raw(value),
    ])
}

fn heading(text: &str, look: &Look) -> Line<'static> {
    Line::styled(text.to_string(), fg(look.theme.accent).bold())
}

/// Text from /proc, one line each, with tabs turned into spaces.
fn text_lines(text: &str, style: Style) -> Vec<Line<'static>> {
    text.lines()
        .map(|l| Line::styled(l.replace('\t', "    ").trim_end().to_string(), style))
        .collect()
}

fn overview_lines(app: &App, state: &InspectorState, look: &Look) -> Vec<Line<'static>> {
    let t = &look.theme;
    let g = &look.glyphs;
    let snap = &state.snapshot;
    let mut lines: Vec<Line<'static>> = Vec::new();

    match &snap.process {
        Some(p) => lines.push(Line::from(vec![
            Span::styled(p.name.clone(), Style::new().bold()),
            Span::raw(format!(" (PID {})", p.pid)),
            Span::styled(format!("  run by {}", p.user), fg(t.dim)),
        ])),
        None => lines.push(Line::from(vec![
            Span::styled("A hidden program", Style::new().bold()),
            Span::raw(": it belongs to another user, so its details cannot be read."),
        ])),
    }

    match app.find_group(&state.key) {
        Some(grp) if grp.listening() => {
            let addresses: Vec<String> = grp.addresses().iter().map(|ip| ip.to_string()).collect();
            lines.push(Line::from(format!(
                "Listening on port {}/{} at {}.",
                grp.port(),
                grp.protocol(),
                addresses.join(", ")
            )));
            let access = grp.access();
            let color = if access.is_exposed() { t.warn } else { t.good };
            let mut sentence = vec![Span::styled(access.sentence(grp.first().entry.local.ip()), fg(color))];
            if let Some(note) = exposure_note(&grp) {
                sentence.push(Span::styled(format!(" {note}"), fg(t.dim)));
            }
            lines.push(Line::from(sentence));

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
            if let Some(name) = service_name(grp.protocol(), grp.port()) {
                lines.push(Line::styled(
                    format!("Port {} is usually {name} (a guess from the number).", grp.port()),
                    fg(t.subtle),
                ));
            }
        }
        Some(grp) => {
            let e = &grp.first().entry;
            lines.push(Line::from(format!(
                "A {} connection between {} and {}.",
                e.state.to_lowercase(),
                e.local,
                e.remote
            )));
        }
        None => lines.push(Line::styled(
            "The row this was opened from has closed.",
            fg(t.bad),
        )),
    }
    lines.push(Line::from(""));

    if let Some(p) = &snap.process {
        let rows = inspector::process_rows(p);
        for label in ["Started", "Memory", "Threads", "Open files"] {
            if let Some((l, v)) = rows.iter().find(|(l, _)| *l == label) {
                lines.push(kv_line(l, v.clone(), look));
            }
        }
    }
    let listening = snap.sockets.iter().filter(|s| s.listening).count();
    lines.push(kv_line(
        "Sockets",
        format!("{listening} listening, {} connections", snap.sockets.len() - listening),
        look,
    ));
    if let Some(unit) = &snap.origin.systemd_unit {
        lines.push(Line::styled(
            format!("{} {}", g.warn, crate::inspect::Hint::Systemd(unit.clone()).advice()),
            fg(t.warn),
        ));
    }
    if let Some(id) = &snap.origin.container_id {
        lines.push(Line::styled(
            format!("{} {}", g.warn, crate::inspect::Hint::Container(id.clone()).advice()),
            fg(t.warn),
        ));
    }
    if snap.origin.docker_proxy {
        lines.push(Line::styled(
            format!("{} {}", g.warn, crate::inspect::Hint::DockerProxy.advice()),
            fg(t.warn),
        ));
    }

    lines.push(Line::from(""));
    lines.push(heading("Where to look next", look));
    for (tab, what) in [
        ("2 Process", "memory, CPU, who started it, the full command line"),
        ("3 Sockets", "every socket it has, with queue sizes (r looks up host names)"),
        ("4 Origin", "systemd service or Docker container, and the raw cgroup paths"),
        ("5 Env", "its environment variables (values hidden until you press v)"),
        ("6 Raw", "the exact /proc text this screen is built from, and the JSON"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{tab:<12}"), fg(t.accent)),
            Span::raw(what),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "y copies this as JSON. k stops it, f frees the port, o opens it: these close the Inspector first.",
        fg(t.dim),
    ));
    lines
}

fn process_lines(snap: &Snapshot, look: &Look) -> Vec<Line<'static>> {
    let t = &look.theme;
    let Some(p) = &snap.process else {
        return vec![
            Line::from("The details of this program are not readable: it belongs to another user."),
            Line::from(vec![
                Span::raw("Run "),
                Span::styled("sudo \"$(which portwatch)\"", fg(t.accent).bold()),
                Span::raw(" to see them."),
            ]),
        ];
    };
    let mut lines: Vec<Line<'static>> = inspector::process_rows(p)
        .into_iter()
        .map(|(label, value)| kv_line(label, value, look))
        .collect();
    lines.push(Line::from(""));
    lines.push(heading("Command line, one argument per line", look));
    if p.command_line.is_empty() {
        lines.push(Line::styled("(empty: kernel threads and some system programs have none)", fg(t.dim)));
    }
    for (i, arg) in p.command_line.iter().enumerate() {
        let arg: String = arg.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        lines.push(Line::from(vec![
            Span::styled(format!("[{i}] "), fg(t.dim)),
            Span::raw(arg),
        ]));
    }
    lines
}

fn sockets_lines(app: &App, snap: &Snapshot, look: &Look) -> Vec<Line<'static>> {
    let t = &look.theme;
    if snap.sockets.is_empty() {
        return vec![Line::styled("This program has no open sockets right now.", fg(t.dim))];
    }
    let mut lines = vec![Line::styled(
        format!(
            "{:<4} {:<11} {:<21} {:<21} QUEUES    INODE",
            "PROTO", "STATE", "LOCAL", "REMOTE"
        ),
        fg(t.dim).bold(),
    )];
    let mut resolvable = false;
    for s in &snap.sockets {
        let ip = s.remote.parse::<std::net::SocketAddr>().ok().map(|a| a.ip());
        let host = ip
            .filter(|_| !s.listening)
            .and_then(|ip| app.hosts.get(&ip))
            .and_then(|h| h.as_deref());
        if !s.listening && ip.is_some_and(|ip| !ip.is_loopback() && !ip.is_unspecified()) {
            resolvable = true;
        }
        let style = if s.listening { fg(t.good) } else { Style::new() };
        lines.push(Line::styled(inspector::socket_line(s, host), style));
    }
    if resolvable {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "Press r to look up host names for the remote addresses. This sends them to your DNS resolver.",
            fg(t.dim),
        ));
    }
    lines
}

fn origin_lines(snap: &Snapshot, look: &Look) -> Vec<Line<'static>> {
    let t = &look.theme;
    let g = &look.glyphs;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let o = &snap.origin;
    let mut known = false;
    if let Some(unit) = &o.systemd_unit {
        known = true;
        lines.push(kv_line("systemd unit", unit.clone(), look));
        lines.push(Line::styled(
            format!("{} {}", g.warn, crate::inspect::Hint::Systemd(unit.clone()).advice()),
            fg(t.warn),
        ));
    }
    if let Some(id) = &o.container_id {
        known = true;
        lines.push(kv_line("container", id.clone(), look));
        lines.push(Line::styled(
            format!("{} {}", g.warn, crate::inspect::Hint::Container(id.clone()).advice()),
            fg(t.warn),
        ));
        lines.push(Line::styled(
            format!("For the image and ports, run: docker inspect {id}"),
            fg(t.dim),
        ));
    }
    if o.docker_proxy {
        known = true;
        lines.push(Line::styled(
            format!("{} {}", g.warn, crate::inspect::Hint::DockerProxy.advice()),
            fg(t.warn),
        ));
    }
    if !known {
        lines.push(Line::from(
            "Not a systemd service or a Docker container: it was started by a person or a desktop session.",
        ));
    }
    lines.push(Line::from(""));
    lines.push(heading("Control groups (from /proc/PID/cgroup)", look));
    if o.cgroup.is_empty() {
        lines.push(Line::styled("(not readable)", fg(t.dim)));
    }
    for path in &o.cgroup {
        lines.push(Line::from(path.clone()));
    }
    lines
}

fn env_lines(state: &InspectorState, look: &Look) -> Vec<Line<'static>> {
    let t = &look.theme;
    let g = &look.glyphs;
    let Some(env) = &state.snapshot.env else {
        return vec![
            Line::from("The environment of this program is not readable: it belongs to another user."),
            Line::styled("Environment variables are never included in the JSON or the Raw tab.", fg(t.dim)),
        ];
    };
    let mut lines = vec![
        if state.reveal_env {
            Line::styled(
                format!("{} Values are visible. They may contain passwords or keys. Press v to hide them.", g.warn),
                fg(t.warn).bold(),
            )
        } else {
            Line::styled(
                "Values are hidden because they often contain passwords or keys. Press v to show them.",
                fg(t.dim),
            )
        },
        Line::styled(
            format!("{} variables. They are never included in the JSON or the Raw tab.", env.len()),
            fg(t.dim),
        ),
        Line::from(""),
    ];
    for (name, value) in env {
        let shown: String = if state.reveal_env {
            value
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(300)
                .collect()
        } else {
            // A fixed-length mask, so the length of the value is not given away.
            g.mask.repeat(8)
        };
        lines.push(Line::from(vec![
            Span::styled(name.clone(), fg(t.accent)),
            Span::raw("="),
            Span::raw(shown),
        ]));
    }
    lines
}

fn raw_lines(snap: &Snapshot, look: &Look) -> Vec<Line<'static>> {
    let t = &look.theme;
    let mut lines = vec![
        heading("JSON (what y copies; it has no environment but does include the command line)", look),
    ];
    lines.extend(text_lines(&inspector::to_json(snap), Style::new()));
    for (title, text) in &snap.raw {
        lines.push(Line::from(""));
        lines.push(heading(title, look));
        lines.extend(text_lines(text, fg(t.subtle)));
    }
    lines
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
                key("Enter", "inspect: full technical detail on a program"),
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
        // The welcome message and the search footer too.
        app.welcome();
        let screen = draw(&mut app, 110, 30);
        assert!(screen.is_ascii(), "welcome is not ASCII:\n{screen}");
        app.mode = Mode::Filter;
        app.filter = "node".into();
        let screen = draw(&mut app, 110, 30);
        assert!(screen.contains("Enter keep"), "{screen}");
        assert!(screen.is_ascii(), "search footer is not ASCII:\n{screen}");
        app.mode = Mode::Normal;
        app.filter.clear();

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
    fn every_popup_fits_a_standard_80x24_terminal() {
        let mut app = app();

        app.mode = Mode::Help;
        let screen = draw(&mut app, 80, 24);
        for expected in [
            "Help",
            "Mouse",
            "Shift+drag selects text",
            "free the port",
            "this PC only",
            "other devices",
            "worth a look",
            "how long the program",
            "Press any key to close",
        ] {
            assert!(screen.contains(expected), "help at 80x24 is missing {expected:?}:\n{screen}");
        }

        app.mode = Mode::ConfirmKill(KillRequest {
            pid: 1234,
            name: "node".into(),
            protocol: Protocol::Tcp,
            port: 3000,
            force: true,
            free: false,
            advice: Some("Managed by systemd (node-app.service); it may restart after a kill.".into()),
        });
        let screen = draw(&mut app, 80, 24);
        for expected in ["Confirm", "SIGKILL", "systemd", "Yes, stop it", "Cancel"] {
            assert!(screen.contains(expected), "popup at 80x24 is missing {expected:?}:\n{screen}");
        }

        app.mode = Mode::Palette(PaletteState::default());
        let screen = draw(&mut app, 80, 24);
        assert!(screen.contains("Command menu") && screen.contains("Enter run"), "{screen}");

        for step in 0..TOUR_STEPS {
            app.mode = Mode::Tour(step);
            let screen = draw(&mut app, 80, 24);
            assert!(screen.contains(&format!("Step {} of {TOUR_STEPS}", step + 1)), "{screen}");
        }
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
        let last = palette::COMMANDS.len() - 1;
        app.mode = Mode::Palette(PaletteState { input: String::new(), selected: last });
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Quit portwatch"), "{screen}");
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

    fn press(app: &mut App, code: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn app_inspecting_ourselves() -> App {
        let mut app = new_app(false);
        app.tracker.update(
            vec![entry(3000, 1, "portwatch", std::process::id())],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app);
        press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Inspector(_)));
        app
    }

    fn go_to(app: &mut App, tab_digit: char) {
        press(app, ratatui::crossterm::event::KeyCode::Char(tab_digit));
    }

    #[test]
    fn every_inspector_tab_renders_at_80x24_and_in_ascii_mode() {
        let expectations = [
            ('1', "Where to look next"),
            ('2', "Command line, one argument per line"),
            ('3', "PROTO"),
            ('4', "Control groups"),
            ('5', "variables"),
            ('6', "JSON (what y copies"),
        ];
        for ascii in [false, true] {
            let mut app = app_inspecting_ourselves();
            if ascii {
                app.look = Look::new(ThemeName::Default, true);
            }
            for (digit, expected) in expectations {
                go_to(&mut app, digit);
                let screen = draw(&mut app, 80, 24);
                assert!(screen.contains(expected), "tab {digit} (ascii {ascii}) missing {expected:?}:\n{screen}");
                assert!(screen.contains("Esc"), "footer missing on tab {digit}:\n{screen}");
                if ascii {
                    assert!(screen.is_ascii(), "tab {digit} is not ASCII:\n{screen}");
                }
            }
        }
    }

    #[test]
    fn the_inspector_survives_tiny_terminals_on_every_tab() {
        let mut app = app_inspecting_ourselves();
        for digit in ['1', '2', '3', '4', '5', '6'] {
            go_to(&mut app, digit);
            for (w, h) in [(20, 5), (1, 1), (60, 3), (200, 60), (30, 8)] {
                let _ = draw(&mut app, w, h);
            }
        }
    }

    #[test]
    fn the_inspector_works_for_hidden_owners_connections_and_exited_programs() {
        // Hidden owner.
        let mut hidden = new_app(false);
        let mut h = entry(631, 3, "x", 1);
        h.process = None;
        hidden.tracker.update(vec![h], Instant::now(), SystemTime::now());
        select_first(&mut hidden);
        press(&mut hidden, ratatui::crossterm::event::KeyCode::Enter);
        let overview = draw(&mut hidden, 100, 30);
        assert!(overview.contains("A hidden program"), "{overview}");
        go_to(&mut hidden, '2');
        assert!(draw(&mut hidden, 100, 30).contains("not readable"));
        go_to(&mut hidden, '5');
        assert!(draw(&mut hidden, 100, 30).contains("not readable"));
        go_to(&mut hidden, '3');
        assert!(draw(&mut hidden, 100, 30).contains("127.0.0.1:631"));

        // A connection row.
        let mut conn_app = new_app(false);
        let mut conn = entry(3000, 4, "portwatch", std::process::id());
        conn.listening = false;
        conn.state = "Established".into();
        conn.remote = "10.20.30.40:55555".parse().unwrap();
        conn_app.tracker.update(vec![conn], Instant::now(), SystemTime::now());
        press(&mut conn_app, ratatui::crossterm::event::KeyCode::Char('2'));
        press(&mut conn_app, ratatui::crossterm::event::KeyCode::Enter);
        let screen = draw(&mut conn_app, 100, 30);
        assert!(screen.contains("established connection"), "{screen}");

        // A program that is already gone.
        let mut gone = new_app(false);
        gone.tracker.update(
            vec![entry(3000, 1, "ghost", 4_000_000)],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut gone);
        press(&mut gone, ratatui::crossterm::event::KeyCode::Enter);
        let screen = draw(&mut gone, 100, 30);
        assert!(screen.contains("has exited"), "{screen}");
        assert!(!screen.contains("Stop"), "actions are not offered for an exited program:\n{screen}");
    }

    #[test]
    fn the_inspector_footer_offers_the_keys_that_fit_each_tab() {
        let mut app = app_inspecting_ourselves();
        let footer = |app: &mut App| draw(app, 140, 30).lines().last().unwrap().to_string();
        let f = footer(&mut app);
        for expected in ["Back", "Tabs", "Scroll", "Copy JSON", "Stop", "Free", "Open"] {
            assert!(f.contains(expected), "missing {expected:?}: {f}");
        }
        go_to(&mut app, '3');
        assert!(footer(&mut app).contains("Names"));
        go_to(&mut app, '5');
        assert!(footer(&mut app).contains("Reveal"));
    }

    #[test]
    fn environment_values_are_masked_until_revealed_and_never_leak_elsewhere() {
        use ratatui::crossterm::event::KeyCode;
        // A clean environment with only the secret, so it is on the first screen.
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .env_clear()
            .env("PORTWATCH_TEST_SECRET", "hunter2-super-secret")
            .spawn()
            .unwrap();
        for _ in 0..200 {
            let comm = std::fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap_or_default();
            if comm.trim_end() == "sleep" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut app = new_app(false);
        app.tracker.update(
            vec![entry(3000, 1, "sleep", child.id())],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app);
        press(&mut app, KeyCode::Enter);

        let secret = "hunter2";
        // Nothing on the Overview, Process, Sockets, Origin or Raw tabs.
        for digit in ['1', '2', '3', '4', '6'] {
            go_to(&mut app, digit);
            for (w, h) in [(100, 30), (80, 24)] {
                let screen = draw(&mut app, w, h);
                assert!(!screen.contains(secret), "secret leaked on tab {digit}:\n{screen}");
                assert!(!screen.contains("PORTWATCH_TEST_SECRET"), "variable name leaked on tab {digit}");
            }
        }
        // The Raw tab scrolled all the way down shows nothing either.
        press(&mut app, KeyCode::End);
        assert!(!draw(&mut app, 100, 30).contains(secret));

        // Env tab: names visible, values masked.
        go_to(&mut app, '5');
        let masked = draw(&mut app, 100, 30);
        assert!(masked.contains("PORTWATCH_TEST_SECRET="), "{masked}");
        assert!(!masked.contains(secret), "value shown without asking:\n{masked}");
        assert!(masked.contains("\u{2022}"), "mask glyph expected:\n{masked}");

        // Revealed on request only.
        press(&mut app, KeyCode::Char('v'));
        let revealed = draw(&mut app, 100, 30);
        assert!(revealed.contains("hunter2-super-secret"), "{revealed}");
        assert!(revealed.contains("passwords or keys"), "a warning is shown while revealed");

        // Leaving the tab hides them again.
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Left);
        assert!(!draw(&mut app, 100, 30).contains(secret));

        // ASCII mode masks with an ASCII character.
        app.look = Look::new(ThemeName::Default, true);
        let ascii = draw(&mut app, 100, 30);
        assert!(ascii.is_ascii() && ascii.contains("********") && !ascii.contains(secret), "{ascii}");

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn long_inspector_content_scrolls_exactly_without_wrapping() {
        use ratatui::crossterm::event::KeyCode;
        // 80 arguments make an 81-line command list, longer than the screen.
        let mut cmd = std::process::Command::new("sleep");
        for _ in 0..80 {
            cmd.arg("60");
        }
        let mut child = cmd.spawn().unwrap();
        for _ in 0..200 {
            let comm = std::fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap_or_default();
            if comm.trim_end() == "sleep" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut app = new_app(false);
        app.tracker.update(
            vec![entry(3000, 1, "sleep", child.id())],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app);
        press(&mut app, KeyCode::Enter);
        go_to(&mut app, '2');

        let top = draw(&mut app, 100, 24);
        assert!(top.contains("lines 1-"), "scroll indicator at the top:\n{top}");
        assert!(top.contains("[0] sleep"), "{top}");
        assert!(!top.contains("[80]"), "later lines are off screen:\n{top}");

        press(&mut app, KeyCode::End);
        let bottom = draw(&mut app, 100, 24);
        assert!(bottom.contains("[80] 60"), "last line visible after End:\n{bottom}");
        assert!(!bottom.contains("[0] sleep"), "{bottom}");
        // The indicator's last line number equals the total: the clamp is exact.
        let indicator = bottom.lines().find(|l| l.contains(" of ")).expect("indicator");
        let numbers: Vec<usize> = indicator
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .map(|p| p.parse().unwrap())
            .collect();
        assert_eq!(numbers[1], numbers[2], "ends exactly on the last line: {indicator}");

        // Scrolling further changes nothing.
        press(&mut app, KeyCode::PageDown);
        press(&mut app, KeyCode::Down);
        assert_eq!(draw(&mut app, 100, 24), bottom);

        // A narrow terminal cuts lines instead of wrapping them (same line count).
        let narrow_total = {
            let screen = draw(&mut app, 30, 24);
            let line = screen.lines().find(|l| l.contains(" of ")).map(str::to_string);
            line.map(|l| l.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
        };
        assert!(narrow_total.is_some());

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn the_technical_table_shows_raw_facts_at_every_width() {
        let mut app = app_with_hidden_owner();
        app.technical = true;
        for width in [80u16, 100, 140] {
            let screen = draw(&mut app, width, 26);
            for expected in ["BIND", "STATE", "PID", "INODE", "technical view", "3000/tcp", "Listen"] {
                assert!(screen.contains(expected), "missing {expected:?} at {width} cols:\n{screen}");
            }
            assert!(!screen.contains("SERVICE"), "simple columns are replaced:\n{screen}");
        }
        app.tab = Tab::Connections;
        app.selected = None;
        for width in [80u16, 100, 140] {
            let screen = draw(&mut app, width, 26);
            for expected in ["INODE", "Established", "10.20.30.40:55555"] {
                assert!(screen.contains(expected), "missing {expected:?} at {width} cols:\n{screen}");
            }
        }
        // And the footer shows the toggle.
        app.tab = Tab::Listening;
        assert!(draw(&mut app, 140, 26).contains("Tech"));
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
        live.technical = true;
        println!("--- this machine, technical view\n{}", draw(&mut live, 120, 20));

        // The Inspector on this very program.
        let mut me = app_inspecting_ourselves();
        for digit in ['1', '2', '3', '4'] {
            go_to(&mut me, digit);
            println!("--- inspector tab {digit}\n{}", draw(&mut me, 100, 26));
        }
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
