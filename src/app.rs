//! Dashboard state and keyboard handling (no drawing happens here).

use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::inspect::{self, Details};
use crate::kill;
use crate::model::{self, Row, SortKey, Tracker};
use crate::scanner::{self, EntryKey};

const STATUS_SHOWN_FOR: Duration = Duration::from_secs(6);
const PAGE: isize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Listening,
    Connections,
    Events,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Listening, Tab::Connections, Tab::Events];

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn step(self, delta: isize) -> Tab {
        let n = Tab::ALL.len() as isize;
        Tab::ALL[(self.index() as isize + delta).rem_euclid(n) as usize]
    }
}

/// A kill the user has asked for but not yet confirmed.
#[derive(Debug, Clone)]
pub struct KillRequest {
    pub pid: u32,
    pub name: String,
    pub port: u16,
    pub force: bool,
    pub advice: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Mode {
    Normal,
    Filter,
    ConfirmKill(KillRequest),
    Help,
}

pub struct Status {
    pub text: String,
    pub is_error: bool,
    pub at: Instant,
}

pub struct App {
    pub tracker: Tracker,
    pub tab: Tab,
    pub mode: Mode,
    pub filter: String,
    pub sort: SortKey,
    pub reverse: bool,
    /// Selected row, tracked by identity so the cursor does not jump when
    /// rows appear or disappear.
    pub selected: Option<EntryKey>,
    selected_index: usize,
    pub table_state: TableState,
    pub details: Option<Details>,
    pub events_scroll: usize,
    pub status: Option<Status>,
    pub paused: bool,
    pub should_quit: bool,
    /// Set when the main loop should rescan immediately (after a kill).
    pub rescan_now: bool,
    pub is_root: bool,
    pub scan_error: Option<String>,
}

impl App {
    pub fn new(is_root: bool) -> Self {
        App {
            tracker: Tracker::default(),
            tab: Tab::Listening,
            mode: Mode::Normal,
            filter: String::new(),
            sort: SortKey::Port,
            reverse: false,
            selected: None,
            selected_index: 0,
            table_state: TableState::default(),
            details: None,
            events_scroll: 0,
            status: None,
            paused: false,
            should_quit: false,
            rescan_now: false,
            is_root,
            scan_error: None,
        }
    }

    /// One-time greeting in the footer for people opening it the first time.
    pub fn welcome(&mut self) {
        self.set_status("Welcome! ? shows all keys · / searches · k stops a process", false);
    }

    // ---- data -------------------------------------------------------

    pub fn rescan(&mut self) {
        match scanner::scan() {
            Ok(entries) => {
                self.tracker
                    .update(entries, Instant::now(), SystemTime::now());
                self.scan_error = None;
            }
            Err(err) => self.scan_error = Some(err.to_string()),
        }
        self.sync_selection();
        self.refresh_details();
    }

    /// Rows shown in the current table tab, filtered and sorted.
    pub fn visible(&self) -> Vec<&Row> {
        let want_listening = match self.tab {
            Tab::Listening => true,
            Tab::Connections => false,
            Tab::Events => return Vec::new(),
        };
        let mut rows: Vec<&Row> = self
            .tracker
            .rows
            .iter()
            .filter(|r| r.entry.listening == want_listening)
            .filter(|r| model::matches(&r.entry, &self.filter))
            .collect();
        model::sort_rows(&mut rows, self.sort, self.reverse);
        rows
    }

    pub fn selected_row(&self) -> Option<&Row> {
        let key = self.selected?;
        self.tracker.rows.iter().find(|r| r.entry.key() == key)
    }

    /// Open connections to a listening port (shown in the details panel).
    pub fn connections_on_port(&self, port: u16) -> usize {
        self.tracker
            .rows
            .iter()
            .filter(|r| r.closed_at.is_none() && !r.entry.listening)
            .filter(|r| r.entry.local.port() == port)
            .count()
    }

    pub fn status_text(&self, now: Instant) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|s| now.saturating_duration_since(s.at) < STATUS_SHOWN_FOR)
    }

    fn set_status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = Some(Status {
            text: text.into(),
            is_error,
            at: Instant::now(),
        });
    }

    // ---- selection --------------------------------------------------

    /// Keep the selection valid after the visible rows changed.
    fn sync_selection(&mut self) {
        let keys: Vec<EntryKey> = self.visible().iter().map(|r| r.entry.key()).collect();
        if keys.is_empty() {
            self.selected = None;
            self.selected_index = 0;
            return;
        }
        if let Some(sel) = self.selected {
            if let Some(i) = keys.iter().position(|k| *k == sel) {
                self.selected_index = i;
                return;
            }
        }
        // The selected row vanished (or nothing was selected): stay at the
        // same position in the list.
        let i = self.selected_index.min(keys.len() - 1);
        self.selected = Some(keys[i]);
        self.selected_index = i;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.tab == Tab::Events {
            let next = self.events_scroll as isize - delta; // up = older
            self.events_scroll = next.max(0) as usize;
            return;
        }
        let keys: Vec<EntryKey> = self.visible().iter().map(|r| r.entry.key()).collect();
        if keys.is_empty() {
            return;
        }
        let current = self
            .selected
            .and_then(|s| keys.iter().position(|k| *k == s))
            .unwrap_or(self.selected_index.min(keys.len() - 1));
        let next = (current as isize + delta).clamp(0, keys.len() as isize - 1) as usize;
        self.selected = Some(keys[next]);
        self.selected_index = next;
        self.refresh_details();
    }

    fn select_edge(&mut self, last: bool) {
        self.move_selection(if last { isize::MAX / 2 } else { isize::MIN / 2 });
    }

    pub fn refresh_details(&mut self) {
        let target = self.selected_row().and_then(|r| r.entry.process.clone());
        self.details = target.map(|p| inspect::inspect(p.pid, &p.name));
    }

    fn after_view_change(&mut self) {
        self.events_scroll = 0;
        self.sync_selection();
        self.refresh_details();
    }

    // ---- keys -------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Raw mode swallows Ctrl+C, so handle it ourselves in every mode.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // Each handler puts the mode back if it stays in it.
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.handle_normal(key),
            Mode::Filter => self.handle_filter(key),
            Mode::Help => {} // any key closes the help
            Mode::ConfirmKill(request) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm_kill(request),
                _ => self.set_status("Kill cancelled", false),
            },
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.after_view_change();
                }
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-PAGE),
            KeyCode::PageDown => self.move_selection(PAGE),
            KeyCode::Home | KeyCode::Char('g') => self.select_edge(false),
            KeyCode::End | KeyCode::Char('G') => self.select_edge(true),
            KeyCode::Tab | KeyCode::Right => self.switch_tab(1),
            KeyCode::BackTab | KeyCode::Left => self.switch_tab(-1),
            KeyCode::Char('1') => self.set_tab(Tab::Listening),
            KeyCode::Char('2') => self.set_tab(Tab::Connections),
            KeyCode::Char('3') => self.set_tab(Tab::Events),
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.after_view_change();
            }
            KeyCode::Char('r') => {
                self.reverse = !self.reverse;
                self.after_view_change();
            }
            KeyCode::Char('p') => {
                self.paused = !self.paused;
                let text = if self.paused {
                    "Paused: the table is frozen (press p to resume)"
                } else {
                    "Resumed"
                };
                self.set_status(text, false);
                if !self.paused {
                    self.rescan_now = true;
                }
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('k') => self.request_kill(false),
            KeyCode::Char('K') => self.request_kill(true),
            _ => {}
        }
    }

    fn handle_filter(&mut self, key: KeyEvent) {
        self.mode = Mode::Filter;
        match key.code {
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::Normal;
                self.after_view_change();
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.after_view_change();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push(c);
                self.after_view_change();
            }
            _ => {}
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = None;
        self.selected_index = 0;
        self.after_view_change();
    }

    fn switch_tab(&mut self, delta: isize) {
        self.set_tab(self.tab.step(delta));
    }

    // ---- killing ----------------------------------------------------

    fn request_kill(&mut self, force: bool) {
        if self.tab != Tab::Listening {
            self.set_status("Switch to the Listening tab to stop a process", true);
            return;
        }
        let target = self.selected_row().map(|r| {
            (
                r.entry.process.clone(),
                r.entry.local.port(),
                r.entry.user.clone(),
            )
        });
        let Some((process, port, user)) = target else {
            self.set_status("Nothing selected", true);
            return;
        };
        let Some(process) = process else {
            self.set_status(
                format!(
                    "No process info for port {port} (owned by {user}). Run with sudo to manage it"
                ),
                true,
            );
            return;
        };
        if let Some(why) = kill::protected_reason(process.pid, std::process::id()) {
            self.set_status(format!("Refusing to kill: {why}"), true);
            return;
        }
        let advice = self
            .details
            .as_ref()
            .and_then(|d| d.hint.as_ref())
            .map(|h| h.advice());
        self.mode = Mode::ConfirmKill(KillRequest {
            pid: process.pid,
            name: process.name,
            port,
            force,
            advice,
        });
    }

    fn confirm_kill(&mut self, request: KillRequest) {
        let signal = if request.force { "SIGKILL" } else { "SIGTERM" };
        match kill::stop(request.pid, &request.name, request.force) {
            Ok(()) => {
                self.set_status(
                    format!(
                        "Sent {signal} to {} (PID {}) on port {}",
                        request.name, request.pid, request.port
                    ),
                    false,
                );
                self.rescan_now = true;
            }
            Err(err) => self.set_status(format!("Could not stop {}: {err}", request.name), true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{PortEntry, ProcessInfo, Protocol};
    use ratatui::crossterm::event::KeyEvent;

    pub fn listener(port: u16, inode: u64, process: Option<(&str, u32)>) -> PortEntry {
        PortEntry {
            protocol: Protocol::Tcp,
            local: format!("127.0.0.1:{port}").parse().unwrap(),
            remote: "0.0.0.0:0".parse().unwrap(),
            state: "Listen".into(),
            listening: true,
            inode,
            user: "alice".into(),
            process: process.map(|(n, pid)| ProcessInfo {
                pid,
                name: n.into(),
                started_at: None,
            }),
        }
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn app_with(entries: Vec<PortEntry>) -> App {
        let mut app = App::new(false);
        app.tracker
            .update(entries, Instant::now(), SystemTime::now());
        app.sync_selection();
        app
    }

    #[test]
    fn typing_in_filter_does_not_quit_or_kill() {
        let mut app = app_with(vec![listener(80, 1, Some(("nginx", 500)))]);
        press(&mut app, KeyCode::Char('/'));
        for c in ['q', 'k', 'K', 'y'] {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(!app.should_quit);
        assert!(matches!(app.mode, Mode::Filter));
        assert_eq!(app.filter, "qkKy");
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.filter.is_empty());
    }

    #[test]
    fn selection_follows_the_row_when_others_appear() {
        let mut app = app_with(vec![
            listener(3000, 1, Some(("node", 501))),
            listener(5000, 2, Some(("python", 502))),
        ]);
        press(&mut app, KeyCode::Down);
        let selected = app.selected.unwrap();
        assert_eq!(selected.local.port(), 5000);

        // A new port sorts before the selected one; the cursor must stay put.
        app.tracker.update(
            vec![
                listener(1000, 3, None),
                listener(3000, 1, Some(("node", 501))),
                listener(5000, 2, Some(("python", 502))),
            ],
            Instant::now(),
            SystemTime::now(),
        );
        app.sync_selection();
        assert_eq!(app.selected, Some(selected));
        assert_eq!(app.selected_index, 2);
    }

    #[test]
    fn selection_moves_to_neighbour_when_selected_row_is_removed() {
        let mut app = app_with(vec![
            listener(1, 1, None),
            listener(2, 2, None),
            listener(3, 3, None),
        ]);
        press(&mut app, KeyCode::Down); // select port 2
        // Port 2 closes and its "closed" row expires.
        let later = Instant::now() + Duration::from_secs(60);
        let wall = SystemTime::now();
        app.tracker
            .update(vec![listener(1, 1, None), listener(3, 3, None)], later, wall);
        app.tracker
            .update(vec![listener(1, 1, None), listener(3, 3, None)], later + Duration::from_secs(10), wall);
        app.sync_selection();
        assert_eq!(app.selected.unwrap().local.port(), 3);
    }

    #[test]
    fn kill_needs_a_process_and_confirmation() {
        let mut app = app_with(vec![
            listener(80, 1, None),
            listener(3000, 2, Some(("node", 4242))),
        ]);
        // Port 80 has no known owner.
        press(&mut app, KeyCode::Char('k'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.as_ref().unwrap().is_error);

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('k'));
        assert!(matches!(app.mode, Mode::ConfirmKill(_)));
        // Any key other than y cancels.
        press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(!app.rescan_now);
    }

    #[test]
    fn kill_refuses_pid_1_and_connections_tab() {
        let mut app = app_with(vec![listener(22, 1, Some(("systemd", 1)))]);
        press(&mut app, KeyCode::Char('k'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.as_ref().unwrap().text.contains("Refusing"));

        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('k'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.as_ref().unwrap().text.contains("Listening tab"));
    }

    #[test]
    fn confirming_a_kill_stops_the_process_and_reports_it() {
        use std::os::unix::process::ExitStatusExt;
        let mut child = crate::kill::spawn_sleeper();
        let mut app = app_with(vec![listener(3000, 1, Some(("sleep", child.id())))]);

        press(&mut app, KeyCode::Char('k'));
        assert!(matches!(app.mode, Mode::ConfirmKill(_)));
        press(&mut app, KeyCode::Char('y'));

        assert!(matches!(app.mode, Mode::Normal));
        let status = app.status.as_ref().unwrap();
        assert!(!status.is_error, "unexpected error: {}", status.text);
        assert!(status.text.starts_with("Sent SIGTERM to sleep"));
        assert!(app.rescan_now, "should rescan right after a kill");
        assert_eq!(child.wait().unwrap().signal(), Some(15));
    }

    #[test]
    fn a_failed_kill_is_reported_not_fatal() {
        // The PID does not exist, so the process is "already gone".
        let mut app = app_with(vec![listener(3000, 1, Some(("ghost", 4_000_000)))]);
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('y'));
        let status = app.status.as_ref().unwrap();
        assert!(status.is_error);
        assert!(status.text.contains("already exited"));
        assert!(!app.should_quit);
    }

    #[test]
    fn tabs_wrap_around() {
        let mut app = app_with(vec![]);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.tab, Tab::Events);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.tab, Tab::Listening);
    }

    #[test]
    fn ctrl_c_quits_from_any_mode() {
        let mut app = app_with(vec![]);
        press(&mut app, KeyCode::Char('/'));
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }
}
