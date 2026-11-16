//! Dashboard state and keyboard handling (no drawing happens here).

use std::io;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::browser;
use crate::inspect::{self, Details};
use crate::kill;
use crate::model::{self, Group, GroupKey, SortKey, Tracker};
use crate::scanner::{self, Protocol};
use crate::theme::Look;
use crate::verdict;

const STATUS_SHOWN_FOR: Duration = Duration::from_secs(6);
/// How long after "free this port" we wait before saying it is still in use.
const FREE_WAIT: Duration = Duration::from_secs(3);
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

/// A stop the user has asked for but not yet confirmed.
#[derive(Debug, Clone)]
pub struct KillRequest {
    pub pid: u32,
    pub name: String,
    pub protocol: Protocol,
    pub port: u16,
    pub force: bool,
    /// "Free this port": after stopping, check that the port really freed.
    pub free: bool,
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

/// Waiting to see whether a port became free after we stopped its owner.
#[derive(Debug, Clone)]
pub struct PendingFree {
    pub protocol: Protocol,
    pub port: u16,
    pub since: Instant,
}

pub struct App {
    pub tracker: Tracker,
    pub tab: Tab,
    pub mode: Mode,
    pub filter: String,
    pub sort: SortKey,
    pub reverse: bool,
    pub only_exposed: bool,
    pub only_mine: bool,
    pub hide_noise: bool,
    /// Selected row, tracked by identity so the cursor does not jump when
    /// rows appear or disappear.
    pub selected: Option<GroupKey>,
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
    /// The account "mine" refers to (the person at the keyboard).
    pub me: String,
    pub scan_error: Option<String>,
    pub look: Look,
    pub pending_free: Option<PendingFree>,
    open_url: Box<dyn FnMut(&str) -> io::Result<()>>,
}

impl App {
    pub fn new(is_root: bool, me: String) -> Self {
        App {
            tracker: Tracker::default(),
            tab: Tab::Listening,
            mode: Mode::Normal,
            filter: String::new(),
            sort: SortKey::Port,
            reverse: false,
            only_exposed: false,
            only_mine: false,
            hide_noise: false,
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
            me,
            scan_error: None,
            look: Look::default(),
            pending_free: None,
            open_url: Box::new(|url| browser::open(url)),
        }
    }

    pub fn with_look(mut self, look: Look) -> Self {
        self.look = look;
        self
    }

    #[cfg(test)]
    pub fn with_opener(mut self, opener: impl FnMut(&str) -> io::Result<()> + 'static) -> Self {
        self.open_url = Box::new(opener);
        self
    }

    /// One-time greeting in the footer for people opening it the first time.
    pub fn welcome(&mut self) {
        self.set_status("Welcome! ? shows all keys · / searches · k stops a program", false);
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
        self.check_pending_free();
        self.sync_selection();
        self.refresh_details();
    }

    /// Rows shown in the current table tab: merged, filtered and sorted.
    pub fn visible(&self) -> Vec<Group<'_>> {
        let want_listening = match self.tab {
            Tab::Listening => true,
            Tab::Connections => false,
            Tab::Events => return Vec::new(),
        };
        let mut groups = model::group_rows(
            self.tracker
                .rows
                .iter()
                .filter(|r| r.entry.listening == want_listening),
        );
        groups.retain(|g| self.passes_filters(g));
        model::sort_groups(&mut groups, self.sort, self.reverse);
        groups
    }

    fn passes_filters(&self, group: &Group<'_>) -> bool {
        if !model::group_matches(group, &self.filter) {
            return false;
        }
        if self.only_mine && group.user() != self.me {
            return false;
        }
        // "Exposed" and "system noise" only make sense for listening ports.
        if group.listening() {
            if self.only_exposed && !group.access().is_exposed() {
                return false;
            }
            if self.hide_noise && verdict::is_noise(group) {
                return false;
            }
        }
        true
    }

    pub fn selected_group(&self) -> Option<Group<'_>> {
        let key = self.selected.as_ref()?;
        self.visible().into_iter().find(|g| &g.key() == key)
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

    pub fn set_status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = Some(Status {
            text: text.into(),
            is_error,
            at: Instant::now(),
        });
    }

    /// Names of the filters that are switched on, for the table title.
    pub fn active_filters(&self) -> Vec<&'static str> {
        let mut active = Vec::new();
        if self.only_mine {
            active.push("mine only");
        }
        if self.tab == Tab::Listening {
            if self.only_exposed {
                active.push("exposed only");
            }
            if self.hide_noise {
                active.push("system noise hidden");
            }
        }
        active
    }

    // ---- selection --------------------------------------------------

    /// Keep the selection valid after the visible rows changed.
    fn sync_selection(&mut self) {
        let keys: Vec<GroupKey> = self.visible().iter().map(|g| g.key()).collect();
        if keys.is_empty() {
            self.selected = None;
            self.selected_index = 0;
            return;
        }
        if let Some(sel) = &self.selected {
            if let Some(i) = keys.iter().position(|k| k == sel) {
                self.selected_index = i;
                return;
            }
        }
        // The selected row vanished (or nothing was selected): stay at the
        // same position in the list.
        let i = self.selected_index.min(keys.len() - 1);
        self.selected = Some(keys[i].clone());
        self.selected_index = i;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.tab == Tab::Events {
            let next = self.events_scroll as isize - delta; // up = older
            self.events_scroll = next.max(0) as usize;
            return;
        }
        let keys: Vec<GroupKey> = self.visible().iter().map(|g| g.key()).collect();
        if keys.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|s| keys.iter().position(|k| k == s))
            .unwrap_or(self.selected_index.min(keys.len() - 1));
        let next = (current as isize + delta).clamp(0, keys.len() as isize - 1) as usize;
        self.selected = Some(keys[next].clone());
        self.selected_index = next;
        self.refresh_details();
    }

    fn select_edge(&mut self, last: bool) {
        self.move_selection(if last { isize::MAX / 2 } else { isize::MIN / 2 });
    }

    pub fn refresh_details(&mut self) {
        let target = self.selected_group().and_then(|g| g.process().cloned());
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
                _ => self.set_status("Cancelled. Nothing was stopped.", false),
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
            KeyCode::Char('e') => self.toggle_exposed(),
            KeyCode::Char('m') => self.toggle_mine(),
            KeyCode::Char('h') => self.toggle_noise(),
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
            KeyCode::Char('k') => self.request_stop(false, false),
            KeyCode::Char('K') => self.request_stop(true, false),
            KeyCode::Char('f') => self.request_stop(false, true),
            KeyCode::Char('o') => self.request_open(),
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

    // ---- quick filters ----------------------------------------------

    fn toggle_exposed(&mut self) {
        if self.tab != Tab::Listening {
            self.set_status("This filter is for the Listening tab", true);
            return;
        }
        self.only_exposed = !self.only_exposed;
        let text = if self.only_exposed {
            "Showing only ports that other devices on your network can reach (e again to undo)"
        } else {
            "Showing all ports"
        };
        self.set_status(text, false);
        self.after_view_change();
    }

    fn toggle_mine(&mut self) {
        self.only_mine = !self.only_mine;
        let text = if self.only_mine {
            format!("Showing only programs run by {} (m again to undo)", self.me)
        } else {
            "Showing everyone's programs".to_string()
        };
        self.set_status(text, false);
        self.after_view_change();
    }

    fn toggle_noise(&mut self) {
        if self.tab != Tab::Listening {
            self.set_status("This filter is for the Listening tab", true);
            return;
        }
        self.hide_noise = !self.hide_noise;
        let text = if self.hide_noise {
            "Hiding system plumbing: DNS, printing, discovery, time sync (h again to undo)"
        } else {
            "Showing system plumbing again"
        };
        self.set_status(text, false);
        self.after_view_change();
    }

    // ---- stopping and freeing ---------------------------------------

    /// `k` / `K` stop the selected program. `f` does the same but frames it
    /// as freeing the port, and afterwards checks that the port is free.
    fn request_stop(&mut self, force: bool, free: bool) {
        if self.tab != Tab::Listening {
            self.set_status("Switch to the Listening tab to stop a program", true);
            return;
        }
        let target = self.selected_group().map(|g| {
            (
                g.process().cloned(),
                g.protocol(),
                g.port(),
                g.user().to_string(),
            )
        });
        let Some((process, protocol, port, user)) = target else {
            self.set_status("Nothing selected", true);
            return;
        };
        let Some(process) = process else {
            self.set_status(
                format!(
                    "No program info for port {port} (owned by {user}). Run with sudo to manage it"
                ),
                true,
            );
            return;
        };
        if let Some(why) = kill::protected_reason(process.pid, std::process::id()) {
            self.set_status(format!("Refusing to stop it: {why}"), true);
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
            protocol,
            port,
            force,
            free,
            advice,
        });
    }

    fn confirm_kill(&mut self, request: KillRequest) {
        let signal = if request.force { "SIGKILL" } else { "SIGTERM" };
        match kill::stop(request.pid, &request.name, request.force) {
            Ok(()) => {
                if request.free {
                    self.pending_free = Some(PendingFree {
                        protocol: request.protocol,
                        port: request.port,
                        since: Instant::now(),
                    });
                    self.set_status(
                        format!(
                            "Sent {signal} to {} (PID {}). Waiting for port {} to be released...",
                            request.name, request.pid, request.port
                        ),
                        false,
                    );
                } else {
                    self.set_status(
                        format!(
                            "Sent {signal} to {} (PID {}) on port {}",
                            request.name, request.pid, request.port
                        ),
                        false,
                    );
                }
                self.rescan_now = true;
            }
            Err(err) => self.set_status(format!("Could not stop {}: {err}", request.name), true),
        }
    }

    /// After "free this port": report whether the port is really free, or
    /// who still holds it. Called after every scan.
    pub fn check_pending_free(&mut self) {
        let Some(pending) = self.pending_free.clone() else {
            return;
        };
        let mut holders: Vec<String> = self
            .tracker
            .rows
            .iter()
            .filter(|r| r.closed_at.is_none() && r.entry.listening)
            .filter(|r| r.entry.protocol == pending.protocol && r.entry.local.port() == pending.port)
            .map(|r| match &r.entry.process {
                Some(p) => format!("{} (PID {})", p.name, p.pid),
                None => format!("a hidden program owned by {}", r.entry.user),
            })
            .collect();
        holders.sort();
        holders.dedup();

        if holders.is_empty() {
            self.pending_free = None;
            self.set_status(format!("Port {} is free.", pending.port), false);
        } else if pending.since.elapsed() >= FREE_WAIT {
            self.pending_free = None;
            self.set_status(
                format!(
                    "Port {} is still in use by {}. Try K to force stop.",
                    pending.port,
                    holders.join(", ")
                ),
                true,
            );
        }
    }

    // ---- opening in the browser -------------------------------------

    fn request_open(&mut self) {
        if self.is_root {
            self.set_status(
                "Not opening a browser as root. Run portwatch as your normal user for this",
                true,
            );
            return;
        }
        let url = self.selected_group().and_then(|g| browser::url_for(&g));
        let Some(url) = url else {
            self.set_status("Only listening TCP ports can be opened in a browser", true);
            return;
        };
        match (self.open_url)(&url) {
            Ok(()) => self.set_status(format!("Opening {url} in your browser"), false),
            Err(err) => self.set_status(
                format!("Could not open the browser ({err}). Is xdg-open installed?"),
                true,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{PortEntry, ProcessInfo};
    use ratatui::crossterm::event::KeyEvent;
    use std::cell::RefCell;
    use std::rc::Rc;

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
        let mut app = App::new(false, "alice".into());
        app.tracker
            .update(entries, Instant::now(), SystemTime::now());
        app.sync_selection();
        app
    }

    fn selected_port(app: &App) -> u16 {
        app.selected_group().unwrap().port()
    }

    #[test]
    fn typing_in_filter_does_not_quit_or_kill() {
        let mut app = app_with(vec![listener(80, 1, Some(("nginx", 500)))]);
        press(&mut app, KeyCode::Char('/'));
        for c in ['q', 'k', 'K', 'y', 'f', 'o', 'e', 'm', 'h'] {
            press(&mut app, KeyCode::Char(c));
        }
        assert!(!app.should_quit);
        assert!(matches!(app.mode, Mode::Filter));
        assert_eq!(app.filter, "qkKyfoemh");
        assert!(!app.only_exposed && !app.only_mine && !app.hide_noise);
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
        let selected = app.selected.clone().unwrap();
        assert_eq!(selected_port(&app), 5000);

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
        app.tracker.update(
            vec![listener(1, 1, None), listener(3, 3, None)],
            later + Duration::from_secs(10),
            wall,
        );
        app.sync_selection();
        assert_eq!(selected_port(&app), 3);
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
        assert!(app.pending_free.is_none(), "k does not wait for the port");
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
    fn free_waits_for_the_port_and_reports_when_it_is_released() {
        let mut child = crate::kill::spawn_sleeper();
        let mut app = app_with(vec![listener(3000, 1, Some(("sleep", child.id())))]);
        press(&mut app, KeyCode::Char('f'));
        assert!(matches!(&app.mode, Mode::ConfirmKill(r) if r.free));
        press(&mut app, KeyCode::Char('y'));
        child.wait().unwrap();
        assert!(app.pending_free.is_some());
        assert!(app.status.as_ref().unwrap().text.contains("Waiting for port 3000"));

        // Still held on the first check: keep waiting quietly.
        app.check_pending_free();
        assert!(app.pending_free.is_some());

        // The scan no longer shows the listener: the port is free.
        app.tracker.update(vec![], Instant::now(), SystemTime::now());
        app.check_pending_free();
        assert!(app.pending_free.is_none());
        let status = app.status.as_ref().unwrap();
        assert_eq!(status.text, "Port 3000 is free.");
        assert!(!status.is_error);
    }

    #[test]
    fn free_says_who_still_holds_the_port_after_the_wait() {
        let mut app = app_with(vec![
            listener(8080, 1, Some(("worker", 11))),
            listener(8080, 2, Some(("worker", 12))),
        ]);
        app.pending_free = Some(PendingFree {
            protocol: Protocol::Tcp,
            port: 8080,
            since: Instant::now().checked_sub(Duration::from_secs(10)).unwrap(),
        });
        app.check_pending_free();
        assert!(app.pending_free.is_none());
        let status = app.status.as_ref().unwrap();
        assert!(status.is_error);
        assert!(status.text.contains("still in use by worker (PID 11), worker (PID 12)"));
    }

    #[test]
    fn quick_filters_narrow_the_table_and_toggle_off() {
        let mut open = listener(80, 1, Some(("nginx", 5)));
        open.local = "0.0.0.0:80".parse().unwrap();
        open.user = "root".into();
        let mut dns = listener(53, 2, None);
        dns.user = "systemd-resolve".into();
        let mine = listener(3000, 3, Some(("node", 6)));
        let mut app = app_with(vec![open, dns, mine]);
        let ports = |app: &App| -> Vec<u16> { app.visible().iter().map(|g| g.port()).collect() };
        assert_eq!(ports(&app), vec![53, 80, 3000]);

        press(&mut app, KeyCode::Char('e')); // exposed only
        assert_eq!(ports(&app), vec![80]);
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(ports(&app), vec![53, 80, 3000]);

        press(&mut app, KeyCode::Char('m')); // mine only
        assert_eq!(ports(&app), vec![3000]);
        assert_eq!(app.active_filters(), vec!["mine only"]);
        press(&mut app, KeyCode::Char('m'));

        press(&mut app, KeyCode::Char('h')); // hide system noise
        assert_eq!(ports(&app), vec![80, 3000]);
        assert_eq!(app.active_filters(), vec!["system noise hidden"]);
    }

    #[test]
    fn exposed_and_noise_filters_do_not_apply_on_the_connections_tab() {
        let mut app = app_with(vec![listener(80, 1, Some(("nginx", 5)))]);
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('h'));
        assert!(!app.only_exposed && !app.hide_noise);
        assert!(app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn open_in_browser_uses_the_listening_port() {
        let opened = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = Rc::clone(&opened);
        let mut app = app_with(vec![listener(3000, 1, Some(("node", 6)))]).with_opener(move |url| {
            sink.borrow_mut().push(url.to_string());
            Ok(())
        });
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(*opened.borrow(), vec!["http://127.0.0.1:3000".to_string()]);
        assert!(!app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn open_in_browser_is_refused_as_root_and_for_udp() {
        let opened = Rc::new(RefCell::new(0));
        let count = Rc::clone(&opened);
        let mut root = App::new(true, "root".into()).with_opener(move |_| {
            *count.borrow_mut() += 1;
            Ok(())
        });
        root.tracker.update(
            vec![listener(3000, 1, Some(("node", 6)))],
            Instant::now(),
            SystemTime::now(),
        );
        root.sync_selection();
        press(&mut root, KeyCode::Char('o'));
        assert_eq!(*opened.borrow(), 0);
        assert!(root.status.as_ref().unwrap().text.contains("as root"));

        let mut udp = listener(5353, 2, Some(("avahi", 7)));
        udp.protocol = Protocol::Udp;
        let mut app = app_with(vec![udp]).with_opener(|_| panic!("must not open UDP"));
        press(&mut app, KeyCode::Char('o'));
        assert!(app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn a_failing_opener_is_reported_not_fatal() {
        let mut app = app_with(vec![listener(3000, 1, Some(("node", 6)))])
            .with_opener(|_| Err(io::Error::new(io::ErrorKind::NotFound, "no such file")));
        press(&mut app, KeyCode::Char('o'));
        let status = app.status.as_ref().unwrap();
        assert!(status.is_error);
        assert!(status.text.contains("xdg-open"));
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
