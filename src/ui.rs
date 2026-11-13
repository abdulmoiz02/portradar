//! Drawing the dashboard.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, Paragraph, Row as TableRow, Table, Tabs, Wrap,
};

use crate::app::{App, KillRequest, Mode, Tab};
use crate::inspect::human_duration;
use crate::model::{EventKind, RowState};

const DIM: Color = Color::DarkGray;

pub fn render(frame: &mut Frame, app: &mut App) {
    let now = Instant::now();
    let area = frame.area();

    let details_height = if app.tab == Tab::Events { 0 } else { 7 };
    let [header, body, details, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(details_height),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, app, header);
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

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let hidden = hidden_owner_count(app);
    let mut counts = vec![
        Span::raw(format!("{} listening", app.tracker.listening_count())),
        Span::raw(" · "),
        Span::raw(format!("{} connections", app.tracker.connection_count())),
        Span::raw(" · "),
    ];
    if app.paused {
        counts.push(Span::styled("PAUSED", Style::new().yellow().bold()));
    } else {
        counts.push(Span::styled("⟳ 1s", Style::new().fg(DIM)));
    }

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(" portwatch ".bold().cyan()))
        .title(Line::from(counts).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let warning = if hidden > 0 && !app.is_root {
        format!("⚠ {hidden} ports hidden: run sudo \"$(which portwatch)\" to see owners")
    } else if let Some(err) = &app.scan_error {
        format!("⚠ scan failed: {err}")
    } else {
        String::new()
    };
    let warn_width = (warning.chars().count() as u16).min(inner.width.saturating_sub(30));
    let [tabs_area, warn_area] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(warn_width)]).areas(inner);

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
        .highlight_style(Style::new().cyan().bold().add_modifier(Modifier::UNDERLINED))
        .divider("│");
    frame.render_widget(tabs, tabs_area);

    if warn_width > 0 {
        frame.render_widget(
            Paragraph::new(warning)
                .style(Style::new().yellow())
                .alignment(Alignment::Right),
            warn_area,
        );
    }
}

/// Listening ports whose owner we could not determine.
fn hidden_owner_count(app: &App) -> usize {
    app.tracker
        .rows
        .iter()
        .filter(|r| r.closed_at.is_none() && r.entry.listening && r.entry.process.is_none())
        .count()
}

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
            let e = &row.entry;
            let (pid, name) = match &e.process {
                Some(p) => (p.pid.to_string(), p.name.clone()),
                None => ("-".to_string(), "-".to_string()),
            };
            let cells: Vec<Cell> = if listening {
                vec![
                    Cell::from(e.protocol.to_string()),
                    Cell::from(e.local.port().to_string()),
                    Cell::from(e.local.ip().to_string()),
                    Cell::from(pid),
                    Cell::from(name),
                    Cell::from(e.user.clone()),
                ]
            } else {
                vec![
                    Cell::from(e.protocol.to_string()),
                    Cell::from(e.local.to_string()),
                    Cell::from(e.remote.to_string()),
                    Cell::from(e.state.clone()),
                    Cell::from(pid),
                    Cell::from(name),
                    Cell::from(e.user.clone()),
                ]
            };
            let style = match row.state(now) {
                RowState::New => Style::new().green().bold(),
                RowState::Closed => Style::new().red().add_modifier(Modifier::CROSSED_OUT),
                RowState::Normal if e.process.is_none() => Style::new().fg(DIM),
                RowState::Normal => Style::new(),
            };
            TableRow::new(cells).style(style)
        })
        .collect();

    let (header, widths): (Vec<&str>, Vec<Constraint>) = if listening {
        (
            vec!["PROTO", "PORT", "ADDRESS", "PID", "PROCESS", "USER"],
            vec![
                Constraint::Length(5),
                Constraint::Length(6),
                Constraint::Min(18),
                Constraint::Length(8),
                Constraint::Min(14),
                Constraint::Min(10),
            ],
        )
    } else {
        (
            vec!["PROTO", "LOCAL", "REMOTE", "STATE", "PID", "PROCESS", "USER"],
            vec![
                Constraint::Length(5),
                Constraint::Min(18),
                Constraint::Min(22),
                Constraint::Length(12),
                Constraint::Length(8),
                Constraint::Min(12),
                Constraint::Min(8),
            ],
        )
    };

    let mut title = vec![
        Span::raw(format!(
            " {} · sort: {} {} ",
            app.tab.label(),
            app.sort.label(),
            if app.reverse { "▼" } else { "▲" }
        )),
    ];
    if !app.filter.is_empty() {
        title.push(Span::styled(
            format!("· filter: {} ", app.filter),
            Style::new().yellow(),
        ));
    }
    let empty_note = if total == 0 {
        if app.filter.is_empty() {
            " nothing here "
        } else {
            " no matches (Esc clears the filter) "
        }
    } else {
        ""
    };

    let table = Table::new(table_rows, widths)
        .header(TableRow::new(header).style(Style::new().bold().fg(Color::Cyan)))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(Line::from(title))
                .title_bottom(Line::from(empty_note).right_aligned()),
        )
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD))
        .highlight_symbol("▶ ")
        .column_spacing(2);

    // `rows` borrows `app`; its data is copied into `table_rows`, so the
    // borrow ends before we touch the table state.
    drop(rows);
    app.table_state.select(selected_index);
    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_events(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Events: listening ports opening and closing (↑↓ scroll) ");
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
                EventKind::Opened => ("▲ opened", Style::new().green()),
                EventKind::Closed => ("▼ closed", Style::new().red()),
            };
            Line::from(vec![
                Span::styled(local.format("%H:%M:%S  ").to_string(), Style::new().fg(DIM)),
                Span::styled(format!("{marker}  "), style),
                Span::raw(event.text.clone()),
            ])
        })
        .collect();

    let content = if lines.is_empty() {
        Paragraph::new("No changes yet. New or closed listening ports will show up here.")
            .style(Style::new().fg(DIM))
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(block, area);
    frame.render_widget(content, inner);
}

fn render_details(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Details ");

    let mut lines: Vec<Line> = Vec::new();
    match (app.selected_row(), &app.details) {
        (None, _) => lines.push(Line::styled("No row selected", Style::new().fg(DIM))),
        (Some(row), details) => {
            let e = &row.entry;
            match (&e.process, details) {
                (Some(p), Some(d)) => {
                    lines.push(label_line(
                        "Command",
                        if d.cmdline.is_empty() { &p.name } else { &d.cmdline },
                    ));
                    let mut facts = Vec::new();
                    if let Some(secs) = d.running_for_secs {
                        facts.push(format!("running {}", human_duration(secs)));
                    }
                    if e.listening {
                        facts.push(format!(
                            "{} open connections",
                            app.connections_on_port(e.local.port())
                        ));
                    }
                    if let Some(cwd) = &d.cwd {
                        facts.push(format!("dir {cwd}"));
                    }
                    if let Some(exe) = &d.exe {
                        facts.push(format!("exe {exe}"));
                    }
                    lines.push(label_line("Info", &facts.join(" · ")));
                    if let Some(hint) = &d.hint {
                        lines.push(Line::from(vec![
                            Span::styled("⚠ ", Style::new().yellow().bold()),
                            Span::styled(hint.advice(), Style::new().yellow()),
                        ]));
                    }
                }
                (Some(p), None) => lines.push(label_line("Process", &format!("{} (PID {})", p.name, p.pid))),
                (None, _) => {
                    lines.push(label_line(
                        "Owner",
                        &format!(
                            "{} (process hidden: it belongs to another user)",
                            e.user
                        ),
                    ));
                    lines.push(Line::styled(
                        "Run `sudo \"$(which portwatch)\"` to see and manage it.",
                        Style::new().yellow(),
                    ));
                }
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn label_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::new().cyan().bold()),
        Span::raw(value.to_string()),
    ])
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect, now: Instant) {
    let line = if matches!(app.mode, Mode::Filter) {
        Line::from(vec![
            Span::styled("/", Style::new().yellow().bold()),
            Span::raw(app.filter.clone()),
            Span::styled("█", Style::new().yellow()),
            Span::styled("   Enter keep · Esc clear", Style::new().fg(DIM)),
        ])
    } else if let Some(status) = app.status_text(now) {
        let style = if status.is_error {
            Style::new().red().bold()
        } else {
            Style::new().green()
        };
        Line::from(Span::styled(status.text.clone(), style))
    } else {
        let keys = [
            ("↑↓", "move"),
            ("←→", "tabs"),
            ("/", "filter"),
            ("s", "sort"),
            ("k", "stop"),
            ("p", "pause"),
            ("?", "help"),
            ("q", "quit"),
        ];
        let mut spans = Vec::new();
        for (key, what) in keys {
            spans.push(Span::styled(key, Style::new().cyan().bold()));
            spans.push(Span::styled(format!(" {what}  "), Style::new().fg(DIM)));
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
        "SIGKILL (immediate, cannot be caught)"
    } else {
        "SIGTERM (asks the program to shut down)"
    };
    let mut lines = vec![
        Line::from(vec![
            Span::raw("Stop "),
            Span::styled(request.name.clone(), Style::new().bold()),
            Span::raw(format!(" (PID {}) using port {}?", request.pid, request.port)),
        ]),
        Line::from(""),
        label_line("Signal", signal),
    ];
    if let Some(advice) = &request.advice {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("⚠ ", Style::new().yellow().bold()),
            Span::styled(advice.clone(), Style::new().yellow()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("y", Style::new().green().bold()),
        Span::raw(" yes     "),
        Span::styled("any other key", Style::new().bold()),
        Span::raw(" cancel"),
    ]));

    let height = lines.len() as u16 + 4;
    let popup = centered(area, 66, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().red())
                .title(" Confirm "),
        ),
        popup,
    );
}

fn render_help(frame: &mut Frame, area: Rect) {
    let rows = [
        ("↑ ↓  PgUp PgDn  g G", "move selection"),
        ("← → / Tab / 1 2 3", "switch tab"),
        ("/", "filter by port, process, user, address…"),
        ("Esc", "clear the filter"),
        ("s  /  r", "change sort column  /  reverse order"),
        ("k", "stop the selected process (SIGTERM, asks first)"),
        ("K", "force stop (SIGKILL, asks first)"),
        ("p", "pause or resume refreshing"),
        ("q  /  Ctrl+C", "quit"),
    ];
    let mut lines = vec![
        Line::styled("Green rows just opened, red rows just closed.", Style::new().fg(DIM)),
        Line::from(""),
    ];
    for (keys, what) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("{keys:<22}"), Style::new().cyan().bold()),
            Span::raw(what),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("Press any key to close", Style::new().fg(DIM)));

    let popup = centered(area, 72, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Help "),
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
        app.rescan_now = false;
        // Select the first row without scanning /proc.
        app.selected = app.visible().first().map(|r| r.entry.key());
        app
    }

    #[test]
    fn renders_header_rows_and_footer() {
        let mut app = app();
        let screen = draw(&mut app, 110, 28);
        assert!(screen.contains("portwatch"));
        assert!(screen.contains("Listening"));
        assert!(screen.contains("Connections"));
        assert!(screen.contains("3000"));
        assert!(screen.contains("postgres"));
        assert!(screen.contains("quit"));
        assert!(screen.contains("2 listening"));
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
        let screen = draw(&mut app, 110, 28);
        assert!(screen.contains("Confirm"));
        assert!(screen.contains("SIGTERM"));
        assert!(screen.contains("systemd"));

        app.mode = Mode::Help;
        let screen = draw(&mut app, 110, 28);
        assert!(screen.contains("Help"));
        assert!(screen.contains("Press any key to close"));
    }

    /// Preview the layout as plain text:
    /// `cargo test print_sample_screen -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_sample_screen() {
        let mut app = app();
        println!("{}", draw(&mut app, 110, 22));
        app.mode = Mode::ConfirmKill(KillRequest {
            pid: 900_001,
            name: "node".into(),
            port: 3000,
            force: false,
            advice: Some(crate::inspect::Hint::Systemd("node-app.service".into()).advice()),
        });
        println!("{}", draw(&mut app, 110, 22));
    }

    #[test]
    fn renders_events_tab_and_survives_tiny_terminals() {
        let mut app = app();
        app.tab = Tab::Events;
        let screen = draw(&mut app, 100, 20);
        assert!(screen.contains("No changes yet"));
        // Must not panic on a very small screen.
        app.tab = Tab::Listening;
        let _ = draw(&mut app, 20, 5);
        let _ = draw(&mut app, 1, 1);
    }
}
