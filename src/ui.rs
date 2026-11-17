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
use crate::confetti;
use crate::model::{EventKind, Group, RowState};
use crate::palette;
use crate::radar::{self, Blip, Mark};
use crate::spark::{compress, sparkline};
use crate::tips;
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
    // The radar wants the room, so on shorter screens it gives up the cards.
    let show_cards = area.height >= 24
        && area.width >= 60
        && !(app.tab == Tab::Radar && area.height < 34);
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
        Tab::Radar => render_radar(frame, app, &look, body, now),
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
    render_confetti(frame, app, &look);
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
        format!(" {} portradar ", g.logo),
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

    let series = |s: &std::collections::VecDeque<u32>| s.iter().copied().collect::<Vec<u32>>();
    let cards = [
        ("Open ports", tracker.listening_count(), "listening now", t.accent, series(&app.history.listening)),
        (
            "Exposed",
            exposed,
            if exposed == 0 { "all local only" } else { "open to network" },
            if exposed > 0 { t.warn } else { t.good },
            series(&app.history.exposed),
        ),
        ("Connections", tracker.connection_count(), "active now", t.accent, series(&app.history.connections)),
        (
            "Hidden",
            hidden,
            if hidden_bad { "need sudo to see" } else { "all owners visible" },
            if hidden_bad { t.warn } else { t.good },
            series(&app.history.hidden),
        ),
    ];

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(area);
    for ((title, number, label, color, history), rect) in cards.into_iter().zip(columns.iter()) {
        let block = look
            .block()
            .border_style(fg(color))
            .title(Line::from(format!(" {title} ")).style(fg(color).bold()));
        // The number, then a tiny graph of the last minute (when it fits).
        let digits = number.to_string();
        let room = (rect.width as usize).saturating_sub(2 + digits.chars().count() + 2);
        let mut headline = vec![Span::styled(digits, fg(color).bold())];
        if room >= 4 {
            // All of the history (about a minute), squeezed into the room.
            let columns = room.min(14);
            let graph = sparkline(&compress(&history, columns), columns, &look.glyphs.spark);
            headline.push(Span::raw("  "));
            headline.push(Span::styled(graph, fg(t.subtle)));
        }
        let text = vec![Line::from(headline), Line::styled(label, fg(t.dim))];
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
            Tab::Radar => "4 Radar".to_string(),
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

    let mut block = look.block().border_style(fg(t.dim)).title(Line::from(title));
    if let Some(tip) = tip_line(app, look, area.width, listening) {
        block = block.title_bottom(tip);
    }
    let table = Table::new(table_rows, widths)
        .header(TableRow::new(header).style(fg(t.dim).bold()))
        .block(block)
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
                    Span::styled("sudo \"$(which portradar)\"", fg(t.accent).bold()),
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
            ("T".into(), "Theme", false),
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
            ("T".into(), "Theme", false),
        ],
        Tab::Listening | Tab::Radar => {
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
            // Sorting and the technical table only change the table, so the
            // Radar tab does not offer them.
            if app.tab == Tab::Listening {
                keys.push(("t".into(), "Tech", app.technical));
            }
            keys.push((tabs, "Tabs", false));
            if app.tab == Tab::Listening {
                keys.push(("s".into(), "Sort", false));
            }
            keys.push(("p".into(), "Pause", app.paused));
            keys.push(("T".into(), "Theme", false));
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
    let tab_keys = format!("{}  Tab  1 2 3 4", g.keys_tabs);
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
        ("p  /  T".into(), "pause or resume / switch colour theme"),
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
                Span::styled("sudo \"$(which portradar)\"", fg(t.accent).bold()),
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
            "{:<5} {:<11} {:<21} {:<21} QUEUES  INODE",
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

// ---- radar, tips and confetti ----------------------------------------------

/// The "Did you know?" line for the bottom border of a table or the radar.
/// `about_selection` lets a fact about the selected port win over the general
/// rotation. The text is cut to fit the width.
fn tip_line(app: &App, look: &Look, width: u16, about_selection: bool) -> Option<Line<'static>> {
    if !app.tips {
        return None;
    }
    let selected = if about_selection {
        app.selected_group().map(|g| (g.protocol(), g.port()))
    } else {
        None
    };
    let text = tips::pick(selected, app.elapsed().as_secs());
    let full = format!(" Did you know? {text} ");
    let room = (width as usize).saturating_sub(4);
    let shown: String = if full.chars().count() > room {
        let keep = room.saturating_sub(4);
        format!("{}... ", full.chars().take(keep).collect::<String>())
    } else {
        full
    };
    Some(Line::styled(shown, fg(look.theme.subtle)))
}

/// One dot per listening row, with a stable identity so it never moves.
fn radar_blips(groups: &[Group<'_>], app: &App, now: Instant) -> Vec<Blip> {
    groups
        .iter()
        .map(|grp| {
            let owner: Vec<u8> = match grp.process() {
                Some(p) => p.pid.to_le_bytes().to_vec(),
                None => grp.user().as_bytes().to_vec(),
            };
            let age = if app.animations {
                grp.members
                    .iter()
                    .filter_map(|r| r.first_seen)
                    .map(|t| now.saturating_duration_since(t).as_secs_f32())
                    .reduce(f32::min)
            } else {
                None
            };
            Blip {
                seed: radar::seed(&[
                    grp.protocol().to_string().as_bytes(),
                    &grp.port().to_le_bytes(),
                    &owner,
                ]),
                band: band_of(grp.access()),
                label: grp.port().to_string(),
                level: verdict(grp, &app.me).level,
                state: grp.state(now),
                selected: Some(grp.key()) == app.selected,
                age,
            }
        })
        .collect()
}

fn band_of(access: Access) -> usize {
    match access {
        Access::ThisPcOnly => 0,
        Access::OneNetwork => 1,
        Access::WholeNetwork => 2,
    }
}

/// Smallest width at which the radar gets its side panels.
const PANELS_MIN_WIDTH: u16 = 92;
/// Width of the scope in the middle when the panels are shown.
const SCOPE_MIN_WIDTH: u16 = 46;

fn render_radar(frame: &mut Frame, app: &mut App, look: &Look, area: Rect, now: Instant) {
    let t = &look.theme;
    let g = &look.glyphs;
    let groups = app.visible();
    let blips = radar_blips(&groups, app, now);

    let mut title = vec![
        Span::styled(" Radar ", fg(t.accent).bold()),
        Span::styled(format!("{} further out = more open to others ", g.dot), fg(t.dim)),
    ];
    for name in app.active_filters() {
        title.push(Span::styled(format!("{} {name} ", g.dot), fg(t.warn)));
    }
    let mut block = look.block().border_style(fg(t.dim)).title(Line::from(title));
    if let Some(tip) = tip_line(app, look, area.width, true) {
        block = block.title_bottom(tip);
    }
    let inner = block.inner(area);

    // Wide windows get a panel on each side; narrower ones just the scope.
    let panels = inner.width >= PANELS_MIN_WIDTH && inner.height >= 12;
    let side = if panels {
        ((inner.width - SCOPE_MIN_WIDTH) / 2).clamp(24, 34)
    } else {
        0
    };
    let scope_area = Rect::new(
        inner.x + side,
        inner.y,
        inner.width.saturating_sub(2 * side),
        inner.height,
    );

    let animating = app.animations && !app.paused;
    let seconds = app.elapsed().as_secs_f32();
    let motion = radar::Motion {
        sweep: app.sweep_angle(),
        spin: if animating { seconds * 1.2 } else { 0.0 },
        pulse: if animating { 3.0 + 1.5 * (seconds * 2.4).sin() } else { 3.0 },
    };
    let scope = if look.ascii {
        radar::layout(scope_area.width, scope_area.height, &blips, motion.sweep)
    } else {
        radar::layout_scope(scope_area.width, scope_area.height, &blips, motion)
    };
    if scope.not_shown > 0 {
        block = block.title_bottom(
            Line::styled(format!(" +{} not shown ", scope.not_shown), fg(t.warn)).right_aligned(),
        );
    }

    let lines: Vec<Line<'static>> = if scope.too_small {
        vec![Line::styled("Make the window larger to see the radar.", fg(t.dim))]
    } else {
        let idle_dim = motion.sweep.is_some();
        let mut lines = radar_lines(&scope, look, idle_dim);
        if blips.is_empty()
            && let Some(mid) = lines.get_mut(scope.height as usize / 2 + 2)
        {
            // Nothing to draw: say so in the middle of the empty rings.
            *mid = Line::styled("No ports match the filters.", fg(t.dim)).centered();
        }
        lines
    };

    let left_lines = panels.then(|| radar_left_lines(look, &groups, side as usize, inner.height as usize));
    let right_lines =
        panels.then(|| radar_right_lines(app, look, &groups, side as usize, inner.height as usize));
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), scope_area);
    if let (Some(left), Some(right)) = (left_lines, right_lines) {
        frame.render_widget(
            Paragraph::new(left),
            Rect::new(inner.x + 1, inner.y, side.saturating_sub(1), inner.height),
        );
        frame.render_widget(
            Paragraph::new(right),
            Rect::new(inner.x + inner.width - side + 1, inner.y, side.saturating_sub(2), inner.height),
        );
    }
}

/// Cut a string to `width` characters.
fn fit(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

/// The left panel: how many ports sit in each ring, and a reminder of what
/// the rings and the main keys mean.
fn radar_left_lines(look: &Look, groups: &[Group<'_>], width: usize, height: usize) -> Vec<Line<'static>> {
    let t = &look.theme;
    let g = &look.glyphs;
    let mut counts = [0usize; 3];
    for grp in groups.iter().filter(|grp| !grp.is_closed()) {
        counts[band_of(grp.access())] += 1;
    }
    let inner = width.saturating_sub(2);
    let roomy = height >= 17;
    let mut lines = vec![Line::styled("RANGE", fg(t.accent).bold())];
    lines.push(Line::from(""));
    for (i, (symbol, name, color)) in [
        (g.local, "THIS PC", t.good),
        (g.one, "ONE NETWORK", t.warn),
        (g.whole, "WHOLE NETWORK", t.bad),
    ]
    .into_iter()
    .enumerate()
    {
        let label = fit(&format!("{symbol} {name}"), inner.saturating_sub(3));
        lines.push(Line::from(vec![
            Span::styled(format!("{label:<w$}", w = inner.saturating_sub(3)), fg(color)),
            Span::styled(format!("{:>3}", counts[i]), Style::new().bold()),
        ]));
        if roomy {
            lines.push(Line::from(""));
        }
    }
    lines.push(Line::from(""));
    for text in ["inner ring: only this PC", "middle: one network", "outer: anyone nearby"] {
        lines.push(Line::styled(fit(text, inner), fg(t.dim)));
    }
    lines.push(Line::from(""));
    for text in ["Enter inspect  f free", "k stop  o open"] {
        lines.push(Line::styled(fit(text, inner), fg(t.dim)));
    }
    lines
}

/// The right panel: what is open to the network, then the selected port.
fn radar_right_lines(
    app: &App,
    look: &Look,
    groups: &[Group<'_>],
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let t = &look.theme;
    let g = &look.glyphs;
    let mut lines = vec![Line::styled("OPEN TO THE NETWORK", fg(t.warn).bold()), Line::from("")];

    let mut open: Vec<&Group<'_>> = groups
        .iter()
        .filter(|grp| grp.access().is_exposed() && !grp.is_closed())
        .collect();
    // Anything worth a look first, then by port.
    open.sort_by_key(|grp| (verdict(grp, &app.me).level != Level::Check, grp.port()));

    const CARD_LINES: usize = 6;
    let room = height.saturating_sub(lines.len() + CARD_LINES + 1);
    if open.is_empty() {
        lines.push(Line::styled("nothing: all local only", fg(t.good)));
    }
    let shown = if open.len() > room { room.saturating_sub(1) } else { open.len() };
    for grp in open.iter().take(shown) {
        let check = verdict(grp, &app.me).level == Level::Check;
        let program = grp.process().map(|p| p.name.as_str()).unwrap_or("?");
        let text = fit(&format!("{:<6}{program}", grp.port()), width.saturating_sub(3));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", if check { g.warn } else { g.whole }),
                fg(if check { t.warn } else { t.subtle }),
            ),
            Span::styled(text, fg(if check { t.warn } else { t.subtle })),
        ]));
    }
    if open.len() > shown {
        lines.push(Line::styled(format!("+{} more", open.len() - shown), fg(t.dim)));
    }

    lines.push(Line::from(""));
    lines.push(Line::styled("SELECTED", Style::new().bold()));
    match app.selected_group() {
        Some(grp) => {
            let service = service_name(grp.protocol(), grp.port())
                .map(|s| format!("  {s}"))
                .unwrap_or_default();
            lines.push(Line::styled(
                fit(&format!("{}/{}{service}", grp.port(), grp.protocol()), width.saturating_sub(1)),
                Style::new().bold(),
            ));
            let program = grp.process().map(|p| p.name.clone()).unwrap_or_else(|| "? needs sudo".to_string());
            lines.push(Line::styled(fit(&program, width.saturating_sub(1)), fg(t.subtle)));
            let access = grp.access();
            let color = if access.is_exposed() { t.warn } else { t.good };
            lines.push(Line::styled(access.label().to_string(), fg(color)));
            let v = verdict(&grp, &app.me);
            let (glyph, color) = match v.level {
                Level::Normal => (g.normal, t.good),
                Level::Dev => (g.dev, t.accent),
                Level::Check => (g.warn, t.warn),
                Level::Unknown => ("?", t.dim),
            };
            lines.push(Line::styled(
                fit(&format!("{glyph} {}", level_label(v.level).to_lowercase()), width.saturating_sub(1)),
                fg(color),
            ));
        }
        None => lines.push(Line::styled("nothing selected", fg(t.dim))),
    }
    lines
}

/// The style for each kind of Braille line work.
fn shape_style(shape: radar::Shape, look: &Look) -> Style {
    use radar::Shape;
    let t = &look.theme;
    match shape {
        Shape::Ring(2) | Shape::Pulse => fg(t.ring[1]),
        Shape::Ring(_) => fg(t.ring[0]),
        Shape::Tick(true) => fg(t.tick[1]),
        Shape::Tick(false) => fg(t.tick[0]),
        Shape::Bracket => fg(t.select_fg).bold(),
        Shape::Ripple | Shape::Echo(1) => fg(t.good),
        Shape::Echo(2) => fg(t.beam),
        Shape::Echo(_) => fg(t.ring[0]),
        Shape::Beam => fg(t.beam).bold(),
    }
}

/// What one mark of the radar looks like. `idle_dim` fades ports the sweep
/// has not touched lately (only while the sweep runs).
fn mark_span(mark: Mark, look: &Look, idle_dim: bool) -> Span<'static> {
    let t = &look.theme;
    let g = &look.glyphs;
    match mark {
        Mark::Empty => Span::raw(" "),
        Mark::Ring(_) => Span::styled(g.radar_ring, fg(t.dim)),
        Mark::RingLabel(ch) => Span::styled(ch.to_string(), fg(t.subtle)),
        Mark::Sweep(3) => Span::styled(g.radar_sweep, fg(t.accent).bold()),
        Mark::Sweep(2) => Span::styled(g.radar_sweep, fg(t.accent)),
        Mark::Sweep(_) => Span::styled(g.radar_sweep, fg(t.dim)),
        Mark::Center => Span::styled(g.radar_center, fg(t.accent).bold()),
        Mark::Ripple => Span::styled(g.radar_ripple, fg(t.good)),
        Mark::Label { ch, selected, tone } => {
            if selected {
                Span::styled(ch.to_string(), Style::new().fg(t.select_fg).bg(t.select_bg).bold())
            } else {
                let color = match tone {
                    0 => t.dim,
                    1 => t.subtle,
                    2 => t.good,
                    _ => t.warn,
                };
                Span::styled(ch.to_string(), fg(color))
            }
        }
        Mark::Dot { level, state, hot, glow, selected } => {
            let glyph = match level {
                Level::Normal => g.local,
                Level::Dev => g.dev,
                Level::Check => g.warn,
                Level::Unknown => g.one,
            };
            let color = match (state, level) {
                (RowState::New, _) => t.good,
                (RowState::Closed, _) => t.bad,
                (_, Level::Normal) => t.good,
                (_, Level::Dev) => t.accent,
                (_, Level::Check) => t.warn,
                (_, Level::Unknown) => t.subtle,
            };
            let mut style = if hot || glow >= 3 {
                // Just hit by the sweep: flash it.
                Style::new().fg(t.on_accent).bg(color).bold()
            } else if glow == 2 {
                fg(color).bold()
            } else if glow == 1 || !idle_dim {
                fg(color)
            } else {
                fg(color).add_modifier(Modifier::DIM)
            };
            if selected {
                style = Style::new().fg(t.select_fg).bg(t.select_bg).bold();
            }
            if state == RowState::Closed {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
            Span::styled(glyph, style)
        }
    }
}

/// Turn the pure radar layout into coloured text. Cells with a glow behind
/// the sweep get a tinted background; Braille line work shows through
/// wherever no dot or label sits.
fn radar_lines(radar: &radar::Radar, look: &Look, idle_dim: bool) -> Vec<Line<'static>> {
    radar
        .cells
        .iter()
        .enumerate()
        .map(|(y, row)| {
            let spans: Vec<Span<'static>> = row
                .iter()
                .enumerate()
                .map(|(x, mark)| {
                    let bits = radar.bits[y][x];
                    let mut span = if *mark == Mark::Empty && bits != 0 {
                        let glyph = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
                        let style = radar.shapes[y][x]
                            .map(|s| shape_style(s, look))
                            .unwrap_or_default();
                        Span::styled(glyph.to_string(), style)
                    } else {
                        mark_span(*mark, look, idle_dim)
                    };
                    if span.style.bg.is_none()
                        && let Some(color) = look.wake_color(radar.wake[y][x])
                    {
                        span.style = span.style.bg(color);
                    }
                    span
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// The celebration after a port is really freed: particles drawn straight
/// onto the finished screen, and a short banner. Skipped while a popup or
/// text box is open (see `App::confetti_now`).
fn render_confetti(frame: &mut Frame, app: &App, look: &Look) {
    let Some((seconds, seed, port)) = app.confetti_now() else {
        return;
    };
    let area = frame.area();
    let t = &look.theme;
    let colors = [t.accent, t.good, t.warn, t.bad];
    let particles = confetti::burst(seed, area.width, area.height);
    let buffer = frame.buffer_mut();
    for (x, y, glyph, color) in confetti::cells(&particles, seconds, area.width, area.height) {
        if let Some(cell) = buffer.cell_mut((area.x + x, area.y + y)) {
            cell.set_symbol(look.glyphs.confetti[glyph]);
            cell.set_style(Style::new().fg(colors[color]).bold());
        }
    }
    let banner = format!(" Port {port} is free! ");
    let width = banner.chars().count() as u16;
    if area.height >= 8 && width <= area.width {
        buffer.set_string(
            area.x + (area.width - width) / 2,
            area.y + area.height / 3,
            &banner,
            Style::new().bg(t.accent).fg(t.on_accent).bold(),
        );
    }
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
            " Welcome to portradar ",
            vec![
                Line::from("Think of your computer as a building."),
                Line::from(""),
                Line::from("A port is a numbered door. A program opens a door to wait for"),
                Line::from("visitors: this is called listening."),
                Line::from(""),
                Line::from("portradar shows every open door and who opened it, live."),
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
                key("4", "the Radar tab: your ports as dots on rings"),
                key("T", "try another colour theme"),
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
    use std::time::{Duration, SystemTime};

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
            "portradar",
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
    fn the_radar_footer_leaves_out_keys_that_only_change_the_table() {
        let mut app = app_with_many_ports();
        let footer = draw(&mut app, 220, 30).lines().last().unwrap().to_string();
        assert!(footer.contains("Theme") && footer.contains("Inspect"), "{footer}");
        assert!(!footer.contains("Sort") && !footer.contains("Tech"), "{footer}");
        app.tab = Tab::Listening;
        let footer = draw(&mut app, 220, 30).lines().last().unwrap().to_string();
        assert!(footer.contains("Sort") && footer.contains("Tech"), "{footer}");
    }

    #[test]
    fn a_long_history_is_squeezed_into_the_card_so_a_spike_is_still_visible() {
        let mut app = app();
        for i in 0..60 {
            app.history.record(if i == 3 { 20 } else { 2 }, 1, 1, 0);
        }
        let screen = draw(&mut app, 100, 30);
        let card_row = screen.lines().nth(2).unwrap();
        let first_card: String = card_row.chars().take(26).collect();
        assert!(first_card.contains('\u{2588}'), "the spike (tallest bar) is visible: {first_card}");
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
            started_at: None,
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
            started_at: None,
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
        assert!(!screen.contains("Quit portradar"), "filtered out:\n{screen}");

        app.mode = Mode::Palette(PaletteState { input: "zzzz".into(), selected: 0 });
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("No matching command"), "{screen}");

        // A long result list keeps the highlighted entry on screen.
        let last = palette::COMMANDS.len() - 1;
        app.mode = Mode::Palette(PaletteState { input: String::new(), selected: last });
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Quit portradar"), "{screen}");
    }

    #[test]
    fn the_tour_has_three_readable_pages() {
        let mut app = app();
        let expectations = [
            (0, ["Welcome to portradar", "numbered door", "Step 1 of 3"]),
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
        assert_eq!(app.hit.tabs.len(), 4);

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
            vec![entry(3000, 1, "portradar", std::process::id())],
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
        let mut conn = entry(3000, 4, "portradar", std::process::id());
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
            .env("PORTRADAR_TEST_SECRET", "hunter2-super-secret")
            .spawn()
            .unwrap();
        // Wait until the new program has really started: its name has changed
        // and its environment is readable (both appear a moment after the fork).
        for _ in 0..300 {
            let comm = std::fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap_or_default();
            let env = std::fs::read(format!("/proc/{}/environ", child.id())).unwrap_or_default();
            if comm.trim_end() == "sleep" && !env.is_empty() {
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
                assert!(!screen.contains("PORTRADAR_TEST_SECRET"), "variable name leaked on tab {digit}");
            }
        }
        // The Raw tab scrolled all the way down shows nothing either.
        press(&mut app, KeyCode::End);
        assert!(!draw(&mut app, 100, 30).contains(secret));

        // Env tab: names visible, values masked.
        go_to(&mut app, '5');
        let masked = draw(&mut app, 100, 30);
        assert!(masked.contains("PORTRADAR_TEST_SECRET="), "{masked}");
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
    fn the_no_clipboard_message_is_fully_readable_at_80_columns() {
        let mut app = app_inspecting_ourselves().with_copier(|_, tx| {
            tx.send(crate::clip::CopyResult::NoTool).unwrap();
        });
        press(&mut app, ratatui::crossterm::event::KeyCode::Char('y'));
        app.tick();
        let screen = draw(&mut app, 80, 24);
        let footer = screen.lines().last().unwrap();
        assert!(footer.contains("may be ignored"), "{footer}");
        assert!(footer.contains("Raw tab"), "{footer}");
        assert!(footer.trim_end().ends_with("Raw tab"), "message must not be cut off: {footer}");
    }

    #[test]
    fn socket_columns_line_up_under_their_headers() {
        let mut app = app_inspecting_ourselves();
        go_to(&mut app, '3');
        let screen = draw(&mut app, 100, 24);
        let header = screen.lines().find(|l| l.contains("PROTO")).unwrap();
        let row = screen.lines().find(|l| l.contains("127.0.0.1:3000")).unwrap();
        let col = |line: &str, needle: &str| line.find(needle).unwrap();
        assert_eq!(col(header, "STATE"), col(row, "Listen"), "{header}\n{row}");
        assert_eq!(col(header, "LOCAL"), col(row, "127.0.0.1:3000"), "{header}\n{row}");
        assert_eq!(col(header, "REMOTE"), col(row, "-  "), "{header}\n{row}");
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

    /// Twelve ports only this computer can reach (like a developer's machine)
    /// plus three open to the network, with time frozen and motion off so the
    /// picture is exactly reproducible.
    fn app_with_many_ports() -> App {
        let mut app = new_app(false).with_motion(false, true);
        app.frozen_clock = Some(Duration::from_secs(1));
        let mut entries: Vec<PortEntry> = (0..12)
            .map(|i| entry(6100 + i as u16, 100 + i as u64, "daemon", 800 + i as u32))
            .collect();
        for (i, port) in [8443u16, 9000, 40000].into_iter().enumerate() {
            entries.push(at(entry(port, 200 + i as u64, "service", 900 + i as u32), "0.0.0.0"));
        }
        app.tracker.update(entries, Instant::now(), SystemTime::now());
        select_first(&mut app);
        app.tab = Tab::Radar;
        app
    }

    /// Only the rows inside the radar's border.
    fn radar_text(screen: &str) -> String {
        screen
            .lines()
            .filter(|l| l.starts_with('\u{2502}') || l.starts_with('|'))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_radar_names_its_rings_and_shows_every_port_at_common_sizes() {
        for (w, h) in [(80u16, 24u16), (100, 30), (120, 38)] {
            let mut app = app_with_many_ports();
            let screen = draw(&mut app, w, h);
            for expected in ["Radar", "this PC", "one network", "whole network", "Did you know?"] {
                assert!(screen.contains(expected), "missing {expected:?} at {w}x{h}:\n{screen}");
            }
            assert!(!screen.contains("not shown"), "every port fits at {w}x{h}:\n{screen}");
            // 15 ports: each is one dot ('o' glyph for a normal one in this theme is a
            // filled circle; unknown exposed ones use a diamond).
            let radar = radar_text(&screen);
            let normal = radar.matches('\u{25cf}').count();
            let unknown = radar.matches('\u{25c6}').count();
            // The side panels (wide windows) add one more filled circle: the legend.
            let panels = usize::from(w >= 92);
            assert_eq!(normal, 12 + panels, "twelve local dots at {w}x{h}:\n{screen}");
            assert!(unknown >= 3, "three exposed dots at {w}x{h}:\n{screen}");
        }
    }

    #[test]
    fn the_radar_does_not_move_when_the_sort_changes_or_the_clock_runs() {
        let mut app = app_with_many_ports();
        let before = draw(&mut app, 100, 30);
        app.sort = crate::model::SortKey::Process;
        app.reverse = true;
        let after_sort = draw(&mut app, 100, 30);
        assert_eq!(before, after_sort, "sorting must not reshuffle the radar");

        // Motion on: the sweep changes the picture but never where dots are.
        app.animations = true;
        app.frozen_clock = Some(Duration::from_secs(0));
        let a = radar_text(&draw(&mut app, 100, 30));
        app.frozen_clock = Some(Duration::from_secs(3));
        let b = radar_text(&draw(&mut app, 100, 30));
        let dots = |t: &str| t.matches('\u{25cf}').count() + t.matches('\u{25c6}').count();
        assert_eq!(dots(&a), dots(&b));
    }

    #[test]
    fn the_radar_is_still_when_paused_or_with_animations_off() {
        let mut app = app_with_many_ports();
        app.animations = true;
        app.frozen_clock = Some(Duration::from_secs(1));
        assert!(app.sweep_angle().is_some());
        app.paused = true;
        assert!(app.sweep_angle().is_none());
        app.paused = false;
        app.animations = false;
        assert!(app.sweep_angle().is_none());
        assert!(!draw(&mut app, 100, 30).contains('\u{2591}'), "no sweep glyph when still");
    }

    #[test]
    fn selecting_on_the_radar_shows_that_port_and_the_radar_handles_filters_and_tiny_windows() {
        let mut app = app_with_many_ports();
        press(&mut app, ratatui::crossterm::event::KeyCode::Down);
        let screen = draw(&mut app, 100, 34);
        assert!(screen.contains("Details") || screen.contains("daemon"), "{screen}");

        // A filter that matches nothing explains itself.
        app.filter = "zzz".into();
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("No ports match"), "{screen}");
        app.filter.clear();

        for (w, h) in [(20, 5), (30, 9), (24, 9), (1, 1), (200, 60)] {
            let _ = draw(&mut app, w, h);
        }
        assert!(draw(&mut app, 100, 12).contains("Radar"));
    }

    #[test]
    fn the_cards_show_a_history_graph_that_fits_at_80_columns() {
        let mut app = app();
        for i in 0..30 {
            app.history.record(2 + i % 5, 1, i, 0);
        }
        for width in [80u16, 100, 140] {
            let screen = draw(&mut app, width, 30);
            let cards: String = screen.lines().skip(1).take(4).collect::<Vec<_>>().join("\n");
            assert!(cards.chars().any(|c| ('\u{2581}'..='\u{2588}').contains(&c)), "no graph at {width}:\n{cards}");
            assert!(cards.contains("listening now") && cards.contains("open to network") || cards.contains("all local only"), "{cards}");
        }
        // ASCII mode draws it with plain characters.
        app.look = Look::new(ThemeName::Default, true);
        let screen = draw(&mut app, 80, 30);
        assert!(screen.is_ascii(), "{screen}");
        // Too narrow for a graph: just the number.
        let _ = draw(&mut app, 62, 30);
    }

    #[test]
    fn the_tip_line_gives_a_fact_about_the_selected_port_and_can_be_turned_off() {
        let mut app = new_app(false).with_motion(true, true);
        app.frozen_clock = Some(Duration::from_secs(0));
        app.tracker.update(
            vec![entry(3000, 1, "node", 4), entry(54321, 2, "thing", 5)],
            Instant::now(),
            SystemTime::now(),
        );
        select_first(&mut app); // port 3000
        let screen = draw(&mut app, 140, 30);
        assert!(screen.contains("Did you know? 3000 is a favourite for Node"), "{screen}");

        // A port with no fact falls back to the rotating general tips.
        press(&mut app, ratatui::crossterm::event::KeyCode::Down);
        let screen = draw(&mut app, 140, 30);
        assert!(screen.contains("Did you know? Any program can listen"), "{screen}");

        // At 80 columns it is cut with "..." and never breaks the border.
        let narrow = draw(&mut app, 80, 30);
        let bottom = narrow.lines().find(|l| l.contains("Did you know?")).unwrap();
        assert!(bottom.trim_end().ends_with('\u{256f}'), "border intact: {bottom}");
        assert!(bottom.contains("..."), "{bottom}");

        app.tips = false;
        assert!(!draw(&mut app, 140, 30).contains("Did you know?"));
    }

    #[test]
    fn confetti_appears_only_when_nothing_else_is_open_and_never_in_non_ascii_ascii_mode() {
        let mut app = app();
        app.confetti = Some(crate::app::Confetti { started: Instant::now(), seed: 7, port: 3000 });
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("Port 3000 is free!"), "{screen}");

        // Not over a popup or a text box.
        app.mode = Mode::Help;
        assert!(!draw(&mut app, 100, 30).contains("is free!"));
        app.mode = Mode::Filter;
        assert!(!draw(&mut app, 100, 30).contains("is free!"));
        app.mode = Mode::Normal;

        // Not with animations off.
        app.animations = false;
        assert!(!draw(&mut app, 100, 30).contains("is free!"));
        app.animations = true;

        // ASCII mode: particles and banner are plain ASCII.
        app.look = Look::new(ThemeName::Default, true);
        let screen = draw(&mut app, 100, 30);
        assert!(screen.contains("is free!") && screen.is_ascii(), "{screen}");

        // Over once its time is up.
        app.confetti = Some(crate::app::Confetti {
            started: Instant::now().checked_sub(Duration::from_secs(5)).unwrap(),
            seed: 7,
            port: 3000,
        });
        assert!(!draw(&mut app, 100, 30).contains("is free!"));
        app.tick();
        assert!(app.confetti.is_none());
    }

    #[test]
    fn the_whole_dashboard_including_the_radar_is_ascii_in_ascii_mode() {
        let mut app = app_with_many_ports().with_look(Look::new(ThemeName::Default, true));
        app.animations = true;
        app.tab = Tab::Radar;
        select_first(&mut app);
        for secs in [0u64, 2, 4] {
            app.frozen_clock = Some(Duration::from_secs(secs));
            for (w, h) in [(80, 24), (120, 38)] {
                let screen = draw(&mut app, w, h);
                assert!(screen.is_ascii(), "radar not ASCII at t={secs} {w}x{h}:\n{screen}");
            }
        }
        app.confetti = Some(crate::app::Confetti { started: Instant::now(), seed: 3, port: 5000 });
        let screen = draw(&mut app, 100, 30);
        assert!(screen.is_ascii(), "confetti not ASCII:\n{screen}");
        assert!(screen.contains("Port 5000 is free!"));
    }

    fn draw_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn braille_cells(screen: &str) -> usize {
        screen.chars().filter(|c| ('\u{2800}'..='\u{28ff}').contains(c)).count()
    }

    #[test]
    fn the_radar_is_a_smooth_braille_scope_with_side_panels_on_wide_windows() {
        for (w, h) in [(100u16, 30u16), (120, 38), (160, 44)] {
            let mut app = app_with_many_ports();
            let screen = draw(&mut app, w, h);
            assert!(braille_cells(&screen) > 100, "smooth rings at {w}x{h}:\n{screen}");
            for expected in ["RANGE", "THIS PC", "ONE NETWORK", "WHOLE NETWORK", "OPEN TO THE NETWORK", "SELECTED", "Enter inspect"] {
                assert!(screen.contains(expected), "missing {expected:?} at {w}x{h}:\n{screen}");
            }
            assert!(!screen.contains('\u{2591}'), "no shaded blocks");
        }
        // On a narrower window the scope gets the whole panel and no side panels.
        let mut app = app_with_many_ports();
        let narrow = draw(&mut app, 80, 24);
        assert!(braille_cells(&narrow) > 50, "{narrow}");
        assert!(!narrow.contains("RANGE") && !narrow.contains("OPEN TO THE NETWORK"), "{narrow}");
        assert!(narrow.contains("this PC") && narrow.contains("whole network"), "{narrow}");
    }

    #[test]
    fn the_side_panels_count_the_rings_list_open_ports_and_describe_the_selection() {
        let mut app = app_with_many_ports();
        let screen = draw(&mut app, 120, 38);
        // Twelve local ports and three open ones.
        let range_lines: Vec<&str> = screen.lines().filter(|l| l.contains("THIS PC") || l.contains("ONE NETWORK") || l.contains("WHOLE NETWORK")).collect();
        let numbers: Vec<String> = range_lines
            .iter()
            .map(|l| {
                // The left panel is the first 34 columns inside the border.
                let left: String = l.split("\u{2502}").nth(1).unwrap_or("").chars().take(34).collect();
                left.split_whitespace().last().unwrap_or("").to_string()
            })
            .collect();
        assert!(numbers.contains(&"12".to_string()), "12 in THIS PC: {numbers:?}\n{screen}");
        assert!(numbers.contains(&"3".to_string()), "3 in WHOLE NETWORK: {numbers:?}\n{screen}");
        for port in ["8443", "9000", "40000"] {
            assert!(screen.contains(&format!("\u{25b2} {port}")), "open port {port} is listed:\n{screen}");
        }
        // The selected port (the first row, 6100) is described.
        assert!(screen.contains("6100/tcp") && screen.contains("daemon") && screen.contains("this PC only"), "{screen}");
    }

    #[test]
    fn the_open_to_the_network_list_puts_worrying_ports_first_and_says_when_it_is_cut() {
        let mut app = new_app(false).with_motion(false, true);
        let mut entries: Vec<PortEntry> = Vec::new();
        for i in 0..14u16 {
            entries.push(at(entry(20000 + i, 300 + i as u64, "svc", 700 + i as u32), "0.0.0.0"));
        }
        // A database open to the network is "worth a look" and must come first.
        let mut redis = at(entry(6379, 99, "redis-server", 55), "0.0.0.0");
        redis.user = "redis".into();
        entries.push(redis);
        app.tracker.update(entries, Instant::now(), SystemTime::now());
        select_first(&mut app);
        app.tab = Tab::Radar;
        let screen = draw(&mut app, 120, 30);
        let list: Vec<&str> = screen
            .lines()
            .filter_map(|l| l.split('\u{2502}').nth(1))
            .filter(|l| l.contains("\u{26a0} 6379") || l.contains("\u{25b2} 20000"))
            .collect();
        let first_warning = screen.find("\u{26a0} 6379").expect("the database is listed");
        let first_plain = screen.find("\u{25b2} 20000").expect("plain open ports are listed");
        assert!(first_warning < first_plain, "worth-a-look first: {list:?}\n{screen}");
        assert!(screen.contains("more"), "a cut list says how many are hidden:\n{screen}");
    }

    #[test]
    fn the_radar_in_ascii_mode_has_the_panels_but_no_braille_or_unicode() {
        let mut app = app_with_many_ports().with_look(Look::new(ThemeName::Default, true));
        app.animations = true;
        for secs in [0u64, 3] {
            app.frozen_clock = Some(Duration::from_secs(secs));
            let screen = draw(&mut app, 120, 38);
            assert!(screen.is_ascii(), "{screen}");
            assert!(screen.contains("RANGE") && screen.contains("OPEN TO THE NETWORK"), "{screen}");
            assert_eq!(braille_cells(&screen), 0);
        }
    }

    #[test]
    fn the_sweep_glow_uses_smooth_colour_when_the_terminal_allows_it_and_256_colours_otherwise() {
        use ratatui::style::Color;
        let bgs = |app: &mut App| -> Vec<Color> {
            draw_buffer(app, 120, 38).content().iter().map(|c| c.bg).collect()
        };
        let mut app = app_with_many_ports();
        app.animations = true;
        app.frozen_clock = Some(Duration::from_secs(2));

        app.look.truecolor = true;
        let rich = bgs(&mut app);
        assert!(rich.iter().any(|c| matches!(c, Color::Rgb(..))), "smooth glow in true colour");

        app.look.truecolor = false;
        let plain = bgs(&mut app);
        assert!(!plain.iter().any(|c| matches!(c, Color::Rgb(..))), "no 24-bit colour in 256-colour mode");
        assert!(plain.iter().any(|c| matches!(c, Color::Indexed(22))), "the 256-colour ramp is used");

        // No sweep, no glow.
        app.animations = false;
        app.look.truecolor = true;
        assert!(!bgs(&mut app).iter().any(|c| matches!(c, Color::Rgb(..))));
    }

    #[test]
    fn idle_dots_fade_only_while_the_sweep_runs_and_just_swept_dots_flash() {
        use ratatui::style::Modifier;
        let dim_dots = |app: &mut App| -> usize {
            draw_buffer(app, 120, 38)
                .content()
                .iter()
                .filter(|c| c.symbol() == "\u{25cf}" && c.modifier.contains(Modifier::DIM))
                .count()
        };
        let mut app = app_with_many_ports();
        assert_eq!(dim_dots(&mut app), 0, "a still radar does not dim anything");
        app.animations = true;
        app.frozen_clock = Some(Duration::from_secs(1));
        assert!(dim_dots(&mut app) > 3, "while it sweeps, quiet ports fade");
        // Over one full turn of the sweep, ports are flashed as a filled chip
        // when it hits them.
        let mut flashed = false;
        for half_seconds in 0..12u64 {
            app.frozen_clock = Some(Duration::from_millis(half_seconds * 500));
            flashed |= draw_buffer(&mut app, 120, 38).content().iter().any(|c| {
                c.symbol() == "\u{25cf}"
                    && c.bg != ratatui::style::Color::Reset
                    && c.modifier.contains(Modifier::BOLD)
            });
        }
        assert!(flashed, "some port is flashed as the sweep passes it");
    }

    #[test]
    fn the_theme_key_keeps_the_colour_depth_and_the_radar_survives_every_theme() {
        let mut app = app_with_many_ports();
        app.look.truecolor = true;
        for _ in 0..ThemeName::ALL.len() {
            press(&mut app, ratatui::crossterm::event::KeyCode::Char('T'));
            assert!(app.look.truecolor);
            let screen = draw(&mut app, 120, 38);
            assert!(braille_cells(&screen) > 100);
        }
    }

    #[test]
    fn every_theme_renders() {
        for theme in ThemeName::ALL {
            let mut app = app_with_hidden_owner().with_look(Look::new(theme, false));
            select_first(&mut app);
            for tab in Tab::ALL {
                app.tab = tab;
                let screen = draw(&mut app, 100, 36);
                assert!(screen.contains("portradar"), "{theme:?} {tab:?}");
            }
        }
    }

    #[test]
    fn renders_kill_popup_and_help() {
        let mut app = app();
        app.mode = Mode::ConfirmKill(KillRequest {
            pid: 900_001,
            name: "node".into(),
            started_at: None,
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

    fn xterm_css(color: ratatui::style::Color, fallback: &str) -> String {
        use ratatui::style::Color as C;
        const BASE: [&str; 16] = [
            "#000000", "#cd0000", "#00cd00", "#cdcd00", "#0000ee", "#cd00cd", "#00cdcd", "#e5e5e5",
            "#7f7f7f", "#ff0000", "#00ff00", "#ffff00", "#5c5cff", "#ff00ff", "#00ffff", "#ffffff",
        ];
        let indexed = |n: u8| -> String {
            match n {
                0..=15 => BASE[n as usize].to_string(),
                16..=231 => {
                    let i = n as usize - 16;
                    let level = |v: usize| [0, 95, 135, 175, 215, 255][v];
                    format!("#{:02x}{:02x}{:02x}", level(i / 36), level((i / 6) % 6), level(i % 6))
                }
                _ => {
                    let g = 8 + 10 * (n as usize - 232);
                    format!("#{g:02x}{g:02x}{g:02x}")
                }
            }
        };
        match color {
            C::Reset => fallback.to_string(),
            C::Black => indexed(0),
            C::Red => indexed(1),
            C::Green => indexed(2),
            C::Yellow => indexed(3),
            C::Blue => indexed(4),
            C::Magenta => indexed(5),
            C::Cyan => indexed(6),
            C::Gray => indexed(7),
            C::DarkGray => indexed(8),
            C::LightRed => indexed(9),
            C::LightGreen => indexed(10),
            C::LightYellow => indexed(11),
            C::LightBlue => indexed(12),
            C::LightMagenta => indexed(13),
            C::LightCyan => indexed(14),
            C::White => indexed(15),
            C::Indexed(n) => indexed(n),
            C::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    /// The screen as an HTML page with the real colours, so the design can be
    /// looked at in a browser. Written to `$PORTRADAR_HTML` when it is set.
    fn screen_html(app: &mut App, width: u16, height: u16) -> String {
        use ratatui::style::Modifier as M;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let (default_fg, default_bg) = ("#d0cfcc", "#300a24");
        let mut out = String::from("<pre style=\"margin:0;padding:8px;background:#300a24;color:#d0cfcc;font:13px/1.18 'DejaVu Sans Mono',monospace;width:max-content\">");
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                let (mut fg, mut bg) = (xterm_css(cell.fg, default_fg), xterm_css(cell.bg, default_bg));
                if cell.modifier.contains(M::REVERSED) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let mut style = format!("color:{fg};background:{bg};");
                if cell.modifier.contains(M::BOLD) {
                    style.push_str("font-weight:700;");
                }
                if cell.modifier.contains(M::DIM) {
                    style.push_str("opacity:.6;");
                }
                if cell.modifier.contains(M::CROSSED_OUT) {
                    style.push_str("text-decoration:line-through;");
                }
                let symbol = cell.symbol().replace('&', "&amp;").replace('<', "&lt;");
                out.push_str(&format!("<span style=\"{style}\">{symbol}</span>"));
            }
            out.push('\n');
        }
        out.push_str("</pre>");
        out
    }

    /// `PORTRADAR_HTML=/path/file.html cargo test dump_radar_html -- --ignored`
    #[test]
    #[ignore]
    fn dump_radar_html() {
        let Some(path) = std::env::var_os("PORTRADAR_HTML") else {
            return;
        };
        let mut page = String::from("<!doctype html><meta charset=utf-8><body style=\"margin:0;background:#1d1d20;color:#ddd;font-family:sans-serif\">");
        for (label, secs, w, h) in [("120x38 t=1.7", 1.7, 120u16, 38u16), ("120x38 t=4.4", 4.4, 120, 38), ("160x44 t=2.9", 2.9, 160, 44), ("100x30 t=2.9", 2.9, 100, 30)] {
            let mut live = App::new(nix::unistd::geteuid().is_root(), "me".into());
            live.look.truecolor = std::env::var_os("PORTRADAR_256").is_none();
            live.rescan();
            live.tab = Tab::Radar;
            live.frozen_clock = Some(Duration::from_secs_f32(secs));
            select_first(&mut live);
            page.push_str(&format!("<h4 style=\"margin:10px 8px 2px\">{label} (this machine's real ports)</h4>"));
            page.push_str(&screen_html(&mut live, w, h));
        }
        std::fs::write(path, page).unwrap();
    }

    /// A believable machine with made-up programs, so documentation
    /// screenshots never show real processes or paths.
    fn demo_app(theme: ThemeName) -> App {
        let wall = SystemTime::now();
        let now_secs = wall.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        let proc = |port: u16, inode: u64, name: &str, pid: u32, ip: &str, up: u64, user: &str| {
            let mut e = at(entry(port, inode, name, pid), ip);
            e.user = user.into();
            e.process.as_mut().unwrap().started_at = Some(now_secs - up);
            e
        };
        let hidden = |port: u16, inode: u64, ip: &str, user: &str| {
            let mut e = at(entry(port, inode, "x", 1), ip);
            e.user = user.into();
            e.process = None;
            e
        };
        let conn = |port: u16, inode: u64, name: &str, pid: u32, remote: &str| {
            let mut e = at(entry(port, inode, name, pid), "192.168.1.23");
            e.listening = false;
            e.state = "Established".into();
            e.remote = remote.parse().unwrap();
            e
        };
        let (h, d) = (3_600u64, 86_400u64);
        let base = vec![
            hidden(22, 10, "0.0.0.0", "root"),
            hidden(53, 11, "127.0.0.53", "systemd-resolve"),
            hidden(631, 12, "127.0.0.1", "root"),
            hidden(5432, 13, "127.0.0.1", "postgres"),
            proc(3000, 14, "node", 4821, "127.0.0.1", 2 * h + 300, "alice"),
            proc(6379, 15, "redis-server", 1733, "127.0.0.1", 3 * d, "alice"),
            proc(27017, 16, "mongod", 1802, "0.0.0.0", 3 * d, "alice"),
            conn(40112, 30, "firefox", 5100, "140.82.112.4:443"),
            conn(40118, 31, "firefox", 5100, "151.101.1.69:443"),
            conn(40210, 32, "firefox", 5100, "34.107.243.93:443"),
            conn(51234, 33, "code", 4410, "140.82.113.26:443"),
            conn(41990, 34, "slack", 5620, "52.94.236.248:443"),
            conn(3000, 35, "firefox", 5100, "127.0.0.1:41822"),
            conn(36000, 36, "spotify", 5710, "35.186.224.25:443"),
        ];
        let mut app = new_app(false).with_look(Look::new(theme, false)).with_motion(true, true);
        app.look.truecolor = true;
        app.frozen_clock = Some(Duration::from_secs_f32(2.9));
        // Build a little history for the Events tab: things open and close.
        let t0 = Instant::now() - Duration::from_secs(120);
        let at_t = |secs: u64| (t0 + Duration::from_secs(secs), wall - Duration::from_secs(120 - secs));
        let vite = proc(5173, 17, "node", 5003, "127.0.0.1", 40 * 60, "alice");
        let py = proc(8000, 18, "python3", 5304, "0.0.0.0", 12 * 60, "alice");
        let temp = proc(8765, 19, "python3", 5400, "127.0.0.1", 60, "alice");
        let debug = proc(9229, 20, "node", 4821, "127.0.0.1", 2 * h + 300, "alice");
        let mut set = base.clone();
        let steps: Vec<(u64, Vec<PortEntry>)> = vec![
            (0, set.clone()),
            (30, { set.push(vite.clone()); set.clone() }),
            (55, { set.push(py.clone()); set.push(debug.clone()); set.clone() }),
            (80, { set.push(temp.clone()); set.clone() }),
            (100, { set.pop(); set.clone() }),
            (119, set.clone()),
        ];
        for (secs, entries) in steps {
            let (i, w) = at_t(secs);
            app.tracker.update(entries, i, w);
        }
        for i in 0..60u32 {
            let wobble = |lo: u32, hi: u32| lo + (i * 7 + i / 3) % (hi - lo + 1);
            app.history.record(
                10 + usize::from(i > 25) + usize::from(i > 40),
                2 + usize::from(i > 40),
                wobble(5, 11) as usize,
                4,
            );
        }
        app.tab = Tab::Listening;
        let key = app.visible().into_iter().find(|g| g.first().entry.local.port() == 3000).map(|g| g.key());
        app.selected = key;
        app.details = Some(crate::inspect::Details {
            cmdline: "node server.js --port 3000".into(),
            cwd: Some("/home/alice/projects/shop-api".into()),
            running_for_secs: Some(2 * h + 300),
            hint: None,
        });
        app
    }

    /// Writes the screens the README shows as JSON cell dumps (a script turns
    /// them into PNGs): `PORTRADAR_SHOTS=/dir cargo test dump_screenshots -- --ignored`
    #[test]
    #[ignore]
    fn dump_screenshots() {
        use ratatui::crossterm::event::KeyCode;
        use ratatui::style::Modifier as M;
        let Some(dir) = std::env::var_os("PORTRADAR_SHOTS").map(std::path::PathBuf::from) else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();
        let save = |name: &str, app: &mut App, w: u16, h: u16| {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| render(f, app)).unwrap();
            let buffer = terminal.backend().buffer();
            let mut cells = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    let c = &buffer[(x, y)];
                    let m = c.modifier;
                    cells.push(serde_json::json!([
                        c.symbol(),
                        xterm_css(c.fg, "default"),
                        xterm_css(c.bg, "default"),
                        u8::from(m.contains(M::BOLD))
                            | u8::from(m.contains(M::DIM)) << 1
                            | u8::from(m.contains(M::CROSSED_OUT)) << 2
                            | u8::from(m.contains(M::REVERSED)) << 3
                            | u8::from(m.contains(M::ITALIC)) << 4
                            | u8::from(m.contains(M::UNDERLINED)) << 5,
                    ]));
                }
            }
            let doc = serde_json::json!({ "width": w, "height": h, "cells": cells });
            std::fs::write(dir.join(format!("{name}.json")), doc.to_string()).unwrap();
        };

        let mut app = demo_app(ThemeName::Default);
        save("listening", &mut app, 110, 32);
        app.tab = Tab::Connections;
        app.selected = None;
        save("connections", &mut app, 110, 26);
        app.tab = Tab::Events;
        save("events", &mut app, 110, 22);

        let mut radar = demo_app(ThemeName::Default);
        radar.tab = Tab::Radar;
        save("radar", &mut radar, 126, 38);

        let mut kill = demo_app(ThemeName::Default);
        press(&mut kill, KeyCode::Char('k'));
        save("stop", &mut kill, 110, 28);

        let mut menu = demo_app(ThemeName::Default);
        press(&mut menu, KeyCode::Char(':'));
        save("menu", &mut menu, 100, 26);

        let mut help = demo_app(ThemeName::Default);
        press(&mut help, KeyCode::Char('?'));
        save("help", &mut help, 100, 42);

        let mut tour = demo_app(ThemeName::Default);
        tour.mode = Mode::Tour(0);
        save("tour", &mut tour, 100, 28);

        let mut technical = demo_app(ThemeName::Default);
        press(&mut technical, KeyCode::Char('t'));
        save("technical", &mut technical, 110, 24);

        for theme in [ThemeName::Nord, ThemeName::Dracula, ThemeName::Cyberpunk, ThemeName::Matrix] {
            let mut t = demo_app(theme);
            save(&format!("theme-{}", format!("{theme:?}").to_lowercase()), &mut t, 100, 26);
        }

        // The Inspector needs a real process to read from /proc: a throwaway
        // web server in an empty folder, so no personal data shows.
        let empty = std::env::temp_dir().join(format!("portradar-demo-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let mut child = std::process::Command::new("python3")
            .args(["-m", "http.server", "8000", "--bind", "127.0.0.1"])
            .current_dir(&empty)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("python3 is needed for the Inspector screenshots");
        std::thread::sleep(Duration::from_millis(800));
        let real: Vec<PortEntry> = crate::scanner::scan()
            .unwrap()
            .into_iter()
            .filter(|e| e.listening && e.local.port() == 8000 && e.process.as_ref().is_some_and(|p| p.pid == child.id()))
            .collect();
        assert!(!real.is_empty(), "the demo web server did not start on 8000");
        let mut live = new_app(false).with_look(Look::new(ThemeName::Default, false));
        live.tracker.update(real, Instant::now() - Duration::from_secs(60), SystemTime::now());
        select_first(&mut live);
        press(&mut live, KeyCode::Enter);
        go_to(&mut live, '1');
        save("inspector-overview", &mut live, 100, 28);
        go_to(&mut live, '2');
        save("inspector-process", &mut live, 100, 30);
        go_to(&mut live, '3');
        save("inspector-sockets", &mut live, 100, 22);
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&empty);
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

        // The radar with this machine's real ports (motion off so it is repeatable).
        let mut radar = App::new(nix::unistd::geteuid().is_root(), "me".into()).with_motion(false, true);
        radar.rescan();
        radar.tab = Tab::Radar;
        for (w, h) in [(80u16, 24u16), (120, 38)] {
            println!("--- radar on this machine {w}x{h}\n{}", draw(&mut radar, w, h));
        }

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
