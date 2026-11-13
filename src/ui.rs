//! Drawing the dashboard.
//!
//! Colours use the terminal's own palette (cyan, green, yellow, red) so the
//! dashboard follows your theme; only the title bar and the selected row use
//! fixed greys, always together with an explicit text colour so they stay
//! readable on light and dark themes.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, Paragraph, Row as TableRow, Table, Tabs, Wrap,
};

use crate::app::{App, KillRequest, Mode, Tab};
use crate::inspect::{human_duration, short_duration};
use crate::model::{EventKind, Row, RowState};
use crate::services::{Access, service_name};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const GOOD: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
const BAR_BG: Color = Color::Indexed(236);
const CHIP_BG: Color = Color::Indexed(238);

pub fn render(frame: &mut Frame, app: &mut App) {
    let now = Instant::now();
    let area = frame.area();

    // Short or narrow terminals drop the optional parts: first the summary
    // cards, then the details panel.
    let show_cards = area.height >= 24 && area.width >= 60;
    let details_height = match (app.tab, area.height) {
        (Tab::Events, _) => 0,
        (_, h) if h >= 34 => 7,
        (_, h) if h >= 24 => 5,
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

    render_title_bar(frame, app, title, !show_cards);
    if show_cards {
        render_cards(frame, app, cards);
    }
    render_tabs(frame, app, tabs);
    match app.tab {
        Tab::Events => render_events(frame, app, body),
        _ => render_table(frame, app, body, now),
    }
    if details_height > 0 {
        render_details(frame, app, details);
    }
    render_footer(frame, app, footer, now);

    match &app.mode {
        Mode::ConfirmKill(request) => render_kill_popup(frame, request, area),
        Mode::Help => render_help(frame, area),
        _ => {}
    }
}

// ---- header ------------------------------------------------------------

fn render_title_bar(frame: &mut Frame, app: &App, area: Rect, show_hidden_warning: bool) {
    let bar = Style::new().bg(BAR_BG).fg(Color::White);

    let mut left = vec![Span::styled(
        " ◉ portwatch ",
        Style::new().bg(ACCENT).fg(Color::Black).bold(),
    )];
    if area.width >= 90 {
        left.push(Span::styled(
            "  ports on this computer, live",
            bar.fg(Color::Gray),
        ));
    }

    let mut right: Vec<Span> = Vec::new();
    if show_hidden_warning && !app.is_root && app.tracker.hidden_count() > 0 {
        right.push(Span::styled(
            format!(" ⚠ {} hidden (needs sudo) ", app.tracker.hidden_count()),
            bar.fg(WARN),
        ));
    }
    if let Some(err) = &app.scan_error {
        right.push(Span::styled(format!(" ⚠ scan failed: {err} "), bar.fg(WARN)));
    } else if app.paused {
        right.push(Span::styled(" ⏸ PAUSED ", bar.fg(WARN).bold()));
    } else {
        right.push(Span::styled(" ● live · 1s ", bar.fg(GOOD)));
    }

    frame.render_widget(Block::new().style(bar), area);
    frame.render_widget(Paragraph::new(Line::from(left)).style(bar), area);
    frame.render_widget(
        Paragraph::new(Line::from(right))
            .style(bar)
            .alignment(Alignment::Right),
        area,
    );
}

fn render_cards(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.tracker;
    let exposed = t.exposed_count();
    let hidden = t.hidden_count();
    let hidden_bad = hidden > 0 && !app.is_root;

    let cards = [
        (
            "Open ports",
            t.listening_count(),
            "listening now",
            ACCENT,
        ),
        (
            "Exposed",
            exposed,
            if exposed == 0 {
                "all local only"
            } else {
                "open to network"
            },
            if exposed > 0 { WARN } else { GOOD },
        ),
        ("Connections", t.connection_count(), "active now", ACCENT),
        (
            "Hidden",
            hidden,
            if hidden_bad {
                "need sudo to see"
            } else {
                "all owners visible"
            },
            if hidden_bad { WARN } else { GOOD },
        ),
    ];

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(area);
    for ((title, number, label, color), rect) in cards.into_iter().zip(columns.iter()) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color))
            .title(Line::from(format!(" {title} ")).style(Style::new().fg(color).bold()));
        let text = vec![
            Line::styled(number.to_string(), Style::new().fg(color).bold()),
            Line::styled(label, Style::new().fg(DIM)),
        ];
        frame.render_widget(
            Paragraph::new(text).alignment(Alignment::Center).block(block),
            *rect,
        );
    }
}

fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<String> = Tab::ALL
        .iter()
        .map(|tab| match tab {
            Tab::Listening => format!("1 Listening ({})", app.tracker.listening_count()),
            Tab::Connections => format!("2 Connections ({})", app.tracker.connection_count()),
            Tab::Events => format!("3 Events ({})", app.tracker.events.len()),
        })
        .collect();
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .style(Style::new().fg(DIM))
        .highlight_style(Style::new().bg(ACCENT).fg(Color::Black).bold())
        .padding(" ", " ")
        .divider(" ");
    frame.render_widget(tabs, area);
}

// ---- table ---------------------------------------------------------------

fn render_table(frame: &mut Frame, app: &mut App, area: Rect, now: Instant) {
    let listening = app.tab == Tab::Listening;
    let rows = app.visible();
    let total = rows.len();
    let selected_index = rows
        .iter()
        .position(|r| Some(r.entry.key()) == app.selected);

    let table_rows: Vec<TableRow> = rows
        .iter()
        .map(|row| {
            let cells = if listening {
                listening_cells(row)
            } else {
                connection_cells(row)
            };
            let style = match row.state(now) {
                RowState::New => Style::new().fg(GOOD).bold(),
                RowState::Closed => Style::new().fg(BAD).add_modifier(Modifier::CROSSED_OUT),
                RowState::Normal => Style::new(),
            };
            TableRow::new(cells).style(style)
        })
        .collect();

    let (header, widths): (Vec<&str>, Vec<Constraint>) = if listening {
        (
            vec!["PORT", "SERVICE", "ACCESS", "PROCESS", "USER", "UP"],
            vec![
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
            Style::new().fg(ACCENT).bold(),
        ),
        Span::styled(
            format!(
                "· sorted by {} {} ",
                app.sort.label(),
                if app.reverse { "▼" } else { "▲" }
            ),
            Style::new().fg(DIM),
        ),
    ];
    if !app.filter.is_empty() {
        title.push(Span::styled(
            format!("· search: {} ", app.filter),
            Style::new().fg(WARN),
        ));
    }

    let table = Table::new(table_rows, widths)
        .header(TableRow::new(header).style(Style::new().fg(DIM).bold()))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(DIM))
                .title(Line::from(title)),
        )
        .row_highlight_style(Style::new().bg(CHIP_BG).fg(Color::White).bold())
        .highlight_symbol("▌ ")
        .column_spacing(if area.width >= 100 { 2 } else { 1 });

    // `rows` borrows `app`, but everything the table needs was copied into
    // owned cells above, so the borrow can end before we touch the state.
    drop(rows);
    app.table_state.select(selected_index);
    frame.render_stateful_widget(table, area, &mut app.table_state);

    if total == 0 {
        let message = if app.filter.is_empty() {
            "Nothing here right now.".to_string()
        } else {
            format!("No matches for “{}”. Press Esc to clear the search.", app.filter)
        };
        let inner = Rect::new(
            area.x + 1,
            area.y + area.height / 2,
            area.width.saturating_sub(2),
            1.min(area.height),
        );
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::new().fg(DIM))
                .alignment(Alignment::Center),
            inner,
        );
    }
}

fn process_cell(row: &Row) -> Cell<'static> {
    match &row.entry.process {
        Some(p) => Cell::from(Line::from(vec![
            Span::styled(p.name.clone(), Style::new().bold()),
            Span::styled(format!(" {}", p.pid), Style::new().fg(DIM)),
        ])),
        None => Cell::from(Span::styled("? needs sudo", Style::new().fg(DIM))),
    }
}

fn listening_cells(row: &Row) -> Vec<Cell<'static>> {
    let e = &row.entry;
    let port = Cell::from(Line::from(vec![
        Span::styled(e.local.port().to_string(), Style::new().bold()),
        Span::styled(format!("/{}", e.protocol), Style::new().fg(DIM)),
    ]));
    let service = match service_name(e.protocol, e.local.port()) {
        Some(name) => Cell::from(name),
        None => Cell::from(Span::styled("-", Style::new().fg(DIM))),
    };
    let access = Access::of(e.local.ip());
    let (symbol, color) = match access {
        Access::ThisPcOnly => ("●", GOOD),
        Access::OneNetwork => ("◆", WARN),
        Access::WholeNetwork => ("▲", WARN),
    };
    let access_cell = Cell::from(Span::styled(
        format!("{symbol} {}", access.label()),
        Style::new().fg(color),
    ));
    let up = match e.process.as_ref().and_then(|p| p.running_for_secs()) {
        Some(secs) => Cell::from(short_duration(secs)),
        None => Cell::from(Span::styled("-", Style::new().fg(DIM))),
    };

    vec![
        port,
        service,
        access_cell,
        process_cell(row),
        Cell::from(e.user.clone()),
        up,
    ]
}

fn connection_cells(row: &Row) -> Vec<Cell<'static>> {
    let e = &row.entry;
    let state_color = match e.state.as_str() {
        "Established" => GOOD,
        "SynSent" | "SynRecv" | "CloseWait" => WARN,
        "TimeWait" | "FinWait1" | "FinWait2" | "LastAck" | "Closing" => DIM,
        _ => Color::Reset,
    };
    vec![
        Cell::from(e.protocol.to_string()),
        Cell::from(e.local.to_string()),
        Cell::from(e.remote.to_string()),
        Cell::from(Span::styled(e.state.clone(), Style::new().fg(state_color))),
        process_cell(row),
        Cell::from(e.user.clone()),
    ]
}

// ---- events ----------------------------------------------------------------

fn render_events(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Line::from(vec![
            Span::styled(" Events ", Style::new().fg(ACCENT).bold()),
            Span::styled("· ports opening and closing · ↑↓ scroll ", Style::new().fg(DIM)),
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
                EventKind::Opened => ("▲ opened", Style::new().fg(GOOD)),
                EventKind::Closed => ("▼ closed", Style::new().fg(BAD)),
            };
            Line::from(vec![
                Span::styled(local.format("%H:%M:%S  ").to_string(), Style::new().fg(DIM)),
                Span::styled(format!("{marker}  "), style),
                Span::raw(event.text.clone()),
            ])
        })
        .collect();

    let content = if lines.is_empty() {
        Paragraph::new("No changes yet. When a program starts or stops listening on a port, it shows up here.")
            .style(Style::new().fg(DIM))
            .wrap(Wrap { trim: true })
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(block, area);
    frame.render_widget(content, inner);
}

// ---- details ---------------------------------------------------------------

fn render_details(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let mut title = " Details ".to_string();

    if let Some(row) = app.selected_row() {
        let e = &row.entry;
        let port = e.local.port();
        let access = Access::of(e.local.ip());
        let listening = e.listening;

        // The address is in the title because several rows can share a
        // port number (for example 127.0.0.53:53 and 127.0.0.54:53).
        let ip = e.local.ip();
        title = match &e.process {
            Some(p) if listening => format!(" {} on port {port} · {ip} ", p.name),
            None if listening => format!(" port {port} · {ip} "),
            Some(p) => format!(" {} connection ", p.name),
            None => " connection ".to_string(),
        };

        if row.closed_at.is_some() {
            lines.push(Line::styled(
                "This port just closed: the program stopped listening on it.",
                Style::new().fg(BAD),
            ));
        } else if !listening {
            let who = match &e.process {
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
        } else if let Some(p) = &e.process {
            lines.push(Line::from(vec![
                Span::styled(p.name.clone(), Style::new().bold()),
                Span::raw(format!(" (PID {}) is listening on port {port}. ", p.pid)),
                Span::styled(
                    access.sentence(e.local.ip()),
                    Style::new().fg(if access.is_exposed() { WARN } else { GOOD }),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw(format!("Something is listening on port {port}, but its program belongs to ")),
                Span::styled(e.user.clone(), Style::new().bold()),
                Span::raw(" and is hidden from you. "),
                Span::styled(access.sentence(e.local.ip()), Style::new().fg(if access.is_exposed() { WARN } else { GOOD })),
            ]));
            lines.push(Line::from(vec![
                Span::raw("To see and manage it, run: "),
                Span::styled("sudo \"$(which portwatch)\"", Style::new().fg(ACCENT).bold()),
            ]));
        }

        if let (Some(p), Some(d)) = (&e.process, &app.details) {
            let mut facts: Vec<String> = Vec::new();
            if listening && let Some(name) = service_name(e.protocol, port) {
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
                lines.push(Line::styled(facts.join(" · "), Style::new().fg(Color::Gray)));
            }
            if let Some(hint) = &d.hint {
                lines.push(Line::from(vec![
                    Span::styled("⚠ ", Style::new().fg(WARN).bold()),
                    Span::styled(hint.advice(), Style::new().fg(WARN)),
                ]));
            }
            if !d.cmdline.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("$ ", Style::new().fg(ACCENT).bold()),
                    Span::raw(d.cmdline.clone()),
                ]));
            }
            if let Some(cwd) = &d.cwd {
                lines.push(Line::styled(format!("in {cwd}"), Style::new().fg(DIM)));
            }
        }
    } else {
        lines.push(Line::styled("Select a row with ↑ ↓ to see what it is.", Style::new().fg(DIM)));
    }

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Line::styled(title, Style::new().fg(ACCENT).bold()));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn label_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::new().fg(ACCENT).bold()),
        Span::raw(value.to_string()),
    ])
}

// ---- footer and popups -------------------------------------------------------

fn key_chip(key: &str) -> Span<'static> {
    Span::styled(
        format!(" {key} "),
        Style::new().bg(CHIP_BG).fg(Color::White).bold(),
    )
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect, now: Instant) {
    let line = if matches!(app.mode, Mode::Filter) {
        Line::from(vec![
            Span::styled(" Search ", Style::new().bg(WARN).fg(Color::Black).bold()),
            Span::raw(" "),
            Span::raw(app.filter.clone()),
            Span::styled("█", Style::new().fg(WARN)),
            Span::styled("   Enter keep · Esc clear", Style::new().fg(DIM)),
        ])
    } else if let Some(status) = app.status_text(now) {
        let color = if status.is_error { BAD } else { GOOD };
        Line::from(Span::styled(
            format!(" {} ", status.text),
            Style::new().fg(color).bold(),
        ))
    } else {
        // Most important first: on a narrow terminal the last ones drop off.
        let keys = [
            ("↑↓", "Move"),
            ("/", "Search"),
            ("k", "Stop"),
            ("?", "Help"),
            ("q", "Quit"),
            ("←→", "Tabs"),
            ("s", "Sort"),
            ("p", "Pause"),
        ];
        let mut spans: Vec<Span> = Vec::new();
        let mut width = 0usize;
        for (key, what) in keys {
            let needed = key.chars().count() + 2 + what.chars().count() + 3;
            if width + needed > area.width as usize {
                break;
            }
            width += needed;
            spans.push(key_chip(key));
            spans.push(Span::styled(format!(" {what}  "), Style::new().fg(Color::Gray)));
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

fn render_kill_popup(frame: &mut Frame, request: &KillRequest, area: Rect) {
    let signal = if request.force {
        "SIGKILL: stops it immediately, it cannot save anything"
    } else {
        "SIGTERM: asks it to shut down cleanly"
    };
    let mut lines = vec![
        Line::from(vec![
            Span::raw("Stop "),
            Span::styled(request.name.clone(), Style::new().bold()),
            Span::raw(format!(" (PID {}), which is using port {}?", request.pid, request.port)),
        ]),
        Line::from(""),
        label_line("How", signal),
    ];
    if let Some(advice) = &request.advice {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("⚠ ", Style::new().fg(WARN).bold()),
            Span::styled(advice.clone(), Style::new().fg(WARN)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        key_chip("y"),
        Span::raw(" Yes, stop it     "),
        key_chip("any other key"),
        Span::raw(" Cancel"),
    ]));

    let height = lines.len() as u16 + 4;
    let popup = centered(area, 68, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(BAD))
                .title(Line::styled(" Confirm ", Style::new().fg(BAD).bold())),
        ),
        popup,
    );
}

fn render_help(frame: &mut Frame, area: Rect) {
    let keys = [
        ("↑ ↓  PgUp PgDn  g G", "move the selection"),
        ("← →  Tab  1 2 3", "switch tab"),
        ("/", "search by port, service, program, user..."),
        ("Esc", "clear the search"),
        ("s  /  r", "change the sort column / reverse it"),
        ("k  /  K", "stop the program (polite / force), asks first"),
        ("p", "pause or resume refreshing"),
        ("q  /  Ctrl+C", "quit"),
    ];
    let mut lines = vec![Line::styled(
        "Green rows just opened. Red rows just closed.",
        Style::new().fg(DIM),
    )];
    lines.push(Line::from(""));
    for (key, what) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<22}"), Style::new().fg(ACCENT).bold()),
            Span::raw(what),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("What the columns mean", Style::new().bold()));
    lines.push(Line::from(vec![
        Span::styled("SERVICE  ", Style::new().fg(ACCENT).bold()),
        Span::raw("what that port number is normally used for (a guess)"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("ACCESS   ", Style::new().fg(ACCENT).bold()),
        Span::styled("● this PC only", Style::new().fg(GOOD)),
        Span::raw(": nothing else can connect"),
    ]));
    lines.push(Line::from(vec![
        Span::raw("         "),
        Span::styled("▲ whole network", Style::new().fg(WARN)),
        Span::raw(": other devices can try"),
    ]));
    lines.push(Line::from(vec![
        Span::styled("UP       ", Style::new().fg(ACCENT).bold()),
        Span::raw("how long the program has been running"),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::styled("Press any key to close", Style::new().fg(DIM)));

    let popup = centered(area, 76, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(Line::styled(" Help ", Style::new().fg(ACCENT).bold())),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{PortEntry, ProcessInfo, Protocol};
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

    fn app() -> App {
        let mut app = App::new(true);
        app.tracker.update(
            vec![
                entry(3000, 1, "node", 900_001),
                entry(5432, 2, "postgres", 900_002),
            ],
            Instant::now(),
            SystemTime::now(),
        );
        // Select the first row without scanning /proc.
        app.selected = app.visible().first().map(|r| r.entry.key());
        app
    }

    /// A non-root app with a visible owner, a hidden owner exposed to the
    /// whole network, and one established connection: what a normal user
    /// sees on a normal machine.
    fn app_with_hidden_owner() -> App {
        let mut app = App::new(false);
        let mut hidden = entry(631, 3, "x", 1);
        hidden.process = None;
        hidden.local = "0.0.0.0:631".parse().unwrap();
        let mut conn = entry(3000, 4, "node", 900_001);
        conn.listening = false;
        conn.state = "Established".into();
        conn.remote = "10.20.30.40:55555".parse().unwrap();
        app.tracker.update(
            vec![entry(3000, 1, "node", 900_001), hidden, conn],
            Instant::now(),
            SystemTime::now(),
        );
        app.selected = app.visible().first().map(|r| r.entry.key());
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
        let screen = draw(&mut app, 110, 34);
        for expected in [
            "node on port 3000",
            "is listening on port 3000",
            "Only this computer can reach it",
            "started 2h 5m ago by alice",
            "$ node server.js",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
        }
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
        assert!(screen.contains("No matches for"), "{screen}");
    }

    #[test]
    fn renders_kill_popup_and_help() {
        let mut app = app();
        app.mode = Mode::ConfirmKill(KillRequest {
            pid: 900_001,
            name: "node".into(),
            port: 3000,
            force: false,
            advice: Some("Managed by systemd (x.service)".into()),
        });
        let screen = draw(&mut app, 110, 30);
        assert!(screen.contains("Confirm"));
        assert!(screen.contains("SIGTERM"));
        assert!(screen.contains("systemd"));
        assert!(screen.contains("Yes, stop it"));

        app.mode = Mode::Help;
        let screen = draw(&mut app, 110, 34);
        assert!(screen.contains("Help"));
        assert!(screen.contains("What the columns mean"));
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
        println!("--- 100x34 help\n{}", draw(&mut app, 100, 34));

        // Real data from this machine.
        let mut live = App::new(nix::unistd::geteuid().is_root());
        live.rescan();
        println!("--- this machine, 110x36\n{}", draw(&mut live, 110, 36));
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
