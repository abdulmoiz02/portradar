//! Pure dashboard logic: tracking which sockets appeared or disappeared
//! between scans, plus filtering and sorting. Nothing here touches the
//! terminal or `/proc`, so it is easy to test.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant, SystemTime};

use crate::scanner::{EntryKey, PortEntry, ProcessInfo, Protocol};
use crate::services::{Access, service_name};
use crate::traffic::Speed;

/// How long a freshly opened port is shown in green.
pub const NEW_HIGHLIGHT: Duration = Duration::from_secs(3);
/// How long a closed port stays visible (in red) before it is dropped.
pub const CLOSED_KEEP: Duration = Duration::from_secs(5);
const MAX_EVENTS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Normal,
    New,
    Closed,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub entry: PortEntry,
    /// `None` for rows present at startup, which are not "new".
    pub first_seen: Option<Instant>,
    pub closed_at: Option<Instant>,
}

impl Row {
    pub fn state(&self, now: Instant) -> RowState {
        if self.closed_at.is_some() {
            return RowState::Closed;
        }
        match self.first_seen {
            Some(t) if now.saturating_duration_since(t) < NEW_HIGHLIGHT => RowState::New,
            _ => RowState::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Opened,
    Closed,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub at: SystemTime,
    pub kind: EventKind,
    pub text: String,
}

/// Keeps the current rows and the history of ports opening and closing.
#[derive(Debug, Default)]
pub struct Tracker {
    pub rows: Vec<Row>,
    /// Oldest first.
    pub events: VecDeque<Event>,
    initialized: bool,
}

impl Tracker {
    /// Merge a fresh scan into the tracked rows.
    ///
    /// The first scan only fills the table. Later scans mark new sockets as
    /// "new", keep vanished sockets around briefly as "closed", and record
    /// an event for every *listening* port that opened or closed (ordinary
    /// connections come and go constantly and would drown the log).
    pub fn update(&mut self, scanned: Vec<PortEntry>, now: Instant, wall: SystemTime) {
        let initial = !self.initialized;
        self.initialized = true;

        let mut fresh: HashMap<_, _> = scanned.into_iter().map(|e| (e.key(), e)).collect();
        let old_rows = std::mem::take(&mut self.rows);
        let mut kept = Vec::with_capacity(old_rows.len() + fresh.len());

        for mut row in old_rows {
            match fresh.remove(&row.entry.key()) {
                Some(entry) => {
                    row.entry = entry;
                    kept.push(row);
                }
                None => match row.closed_at {
                    None => {
                        if row.entry.listening {
                            push_event(&mut self.events, wall, EventKind::Closed, &row.entry);
                        }
                        row.closed_at = Some(now);
                        kept.push(row);
                    }
                    Some(t) if now.saturating_duration_since(t) < CLOSED_KEEP => kept.push(row),
                    Some(_) => {}
                },
            }
        }

        // HashMap order is random; sort new rows so events appear in a
        // stable order.
        let mut added: Vec<PortEntry> = fresh.into_values().collect();
        added.sort_by_key(|e| (e.protocol, e.local.port(), e.local.ip()));
        for entry in added {
            if entry.listening && !initial {
                push_event(&mut self.events, wall, EventKind::Opened, &entry);
            }
            kept.push(Row {
                entry,
                first_seen: if initial { None } else { Some(now) },
                closed_at: None,
            });
        }

        self.rows = kept;
    }

    /// Listening ports, as shown in the table: one entry per program per
    /// port, so the IPv4 and IPv6 sockets of one server count once.
    pub fn listening_count(&self) -> usize {
        self.open_listener_groups().len()
    }

    /// Listening ports that something other than this computer may reach.
    pub fn exposed_count(&self) -> usize {
        self.open_listener_groups()
            .iter()
            .filter(|g| g.access().is_exposed())
            .count()
    }

    /// Listening ports whose owning process we are not allowed to see.
    pub fn hidden_count(&self) -> usize {
        self.open_listener_groups()
            .iter()
            .filter(|g| g.process().is_none())
            .count()
    }

    fn open_listener_groups(&self) -> Vec<Group<'_>> {
        group_rows(self.rows.iter().filter(|r| r.entry.listening))
            .into_iter()
            .filter(|g| !g.is_closed())
            .collect()
    }

    pub fn connection_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.closed_at.is_none() && !r.entry.listening)
            .count()
    }
}

fn push_event(events: &mut VecDeque<Event>, at: SystemTime, kind: EventKind, e: &PortEntry) {
    let who = match &e.process {
        Some(p) => format!("{} [{}]", p.name, p.pid),
        None => format!("(owner: {})", e.user),
    };
    let verb = match kind {
        EventKind::Opened => "opened",
        EventKind::Closed => "closed",
    };
    events.push_back(Event {
        at,
        kind,
        text: format!(
            "{} {} {} on {}  {}",
            e.protocol,
            e.local.port(),
            verb,
            e.local.ip(),
            who
        ),
    });
    while events.len() > MAX_EVENTS {
        events.pop_front();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Port,
    Process,
    Pid,
    User,
    /// Shortest-running first, i.e. what started most recently.
    Uptime,
    /// Least exposed first (this PC only, then one network, then all).
    Access,
    /// Fastest TCP connection first (download plus upload).
    Speed,
}

impl SortKey {
    pub fn next(self) -> Self {
        match self {
            SortKey::Port => SortKey::Process,
            SortKey::Process => SortKey::Pid,
            SortKey::Pid => SortKey::User,
            SortKey::User => SortKey::Uptime,
            SortKey::Uptime => SortKey::Access,
            SortKey::Access => SortKey::Speed,
            SortKey::Speed => SortKey::Port,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Port => "port",
            SortKey::Process => "process",
            SortKey::Pid => "pid",
            SortKey::User => "user",
            SortKey::Uptime => "uptime",
            SortKey::Access => "access",
            SortKey::Speed => "speed",
        }
    }
}

/// Which program owns a listening socket, for grouping.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Owner {
    Pid(u32),
    /// The program is hidden from us; all we know is the account it runs as.
    Hidden(String),
}

/// Identity of a table row. Listening sockets of the same program on the
/// same port are merged into one row (for example IPv4 and IPv6); every
/// other socket is its own row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GroupKey {
    Listener {
        protocol: Protocol,
        port: u16,
        owner: Owner,
    },
    Socket(EntryKey),
}

/// One table row: one or more sockets shown together.
///
/// Grouping only changes how rows are *displayed*. The tracker, the event
/// log and the new/closed highlighting still work per socket.
#[derive(Debug, Clone)]
pub struct Group<'a> {
    /// Never empty. Sorted by address.
    pub members: Vec<&'a Row>,
}

impl<'a> Group<'a> {
    pub fn first(&self) -> &'a Row {
        self.members[0]
    }

    pub fn key(&self) -> GroupKey {
        group_key(self.first())
    }

    pub fn protocol(&self) -> Protocol {
        self.first().entry.protocol
    }

    pub fn port(&self) -> u16 {
        self.first().entry.local.port()
    }

    pub fn listening(&self) -> bool {
        self.first().entry.listening
    }

    pub fn process(&self) -> Option<&'a ProcessInfo> {
        self.first().entry.process.as_ref()
    }

    pub fn user(&self) -> &'a str {
        &self.first().entry.user
    }

    /// The most exposed of the members. A row that mixes `127.0.0.1` and
    /// `0.0.0.0` must say "whole network", never understate.
    pub fn access(&self) -> Access {
        self.members
            .iter()
            .map(|r| Access::of(r.entry.local.ip()))
            .max()
            .unwrap_or(Access::ThisPcOnly)
    }

    pub fn addresses(&self) -> Vec<IpAddr> {
        let mut ips: Vec<IpAddr> = self.members.iter().map(|r| r.entry.local.ip()).collect();
        ips.sort();
        ips.dedup();
        ips
    }

    pub fn is_closed(&self) -> bool {
        self.members.iter().all(|r| r.closed_at.is_some())
    }

    pub fn state(&self, now: Instant) -> RowState {
        if self.is_closed() {
            RowState::Closed
        } else if self.members.iter().any(|r| r.state(now) == RowState::New) {
            RowState::New
        } else {
            RowState::Normal
        }
    }
}

fn group_key(row: &Row) -> GroupKey {
    let e = &row.entry;
    if !e.listening {
        return GroupKey::Socket(e.key());
    }
    GroupKey::Listener {
        protocol: e.protocol,
        port: e.local.port(),
        owner: match &e.process {
            Some(p) => Owner::Pid(p.pid),
            None => Owner::Hidden(e.user.clone()),
        },
    }
}

/// Merge rows into display groups, keeping first-seen order.
///
/// Different PIDs are never merged. If part of a group has closed but part
/// is still open, only the open sockets are kept, so a half-closed pair
/// does not look like two ports.
pub fn group_rows<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Vec<Group<'a>> {
    let mut index: HashMap<GroupKey, usize> = HashMap::new();
    let mut groups: Vec<Vec<&'a Row>> = Vec::new();
    for row in rows {
        let slot = *index.entry(group_key(row)).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(row);
    }
    groups
        .into_iter()
        .map(|mut members| {
            if members.iter().any(|r| r.closed_at.is_none()) {
                members.retain(|r| r.closed_at.is_none());
            }
            members.sort_by_key(|r| (r.entry.local.ip(), r.entry.local.port()));
            Group { members }
        })
        .collect()
}

pub fn sort_groups(
    groups: &mut [Group<'_>],
    key: SortKey,
    reverse: bool,
    speeds: &HashMap<u64, Speed>,
) {
    // Always start from port order so ties are stable.
    groups.sort_by_key(|g| (g.protocol(), g.port(), g.first().entry.local.ip()));
    match key {
        SortKey::Port => {}
        SortKey::Process => groups.sort_by_cached_key(|g| match g.process() {
            Some(p) => (0, p.name.to_lowercase()),
            None => (1, String::new()), // unknown owners last
        }),
        SortKey::Pid => groups.sort_by_key(|g| match g.process() {
            Some(p) => (0, p.pid),
            None => (1, 0),
        }),
        SortKey::User => groups.sort_by_cached_key(|g| g.user().to_lowercase()),
        SortKey::Uptime => groups.sort_by_key(|g| match g.process().and_then(|p| p.started_at) {
            // Larger start time = started later = shorter uptime = first.
            Some(started) => (0, std::cmp::Reverse(started)),
            None => (1, std::cmp::Reverse(0)),
        }),
        SortKey::Access => groups.sort_by_key(|g| g.access()),
        SortKey::Speed => groups.sort_by(|a, b| {
            let total = |g: &Group<'_>| {
                speeds.get(&g.first().entry.inode).map_or(0.0, |s| s.down + s.up)
            };
            total(b).total_cmp(&total(a))
        }),
    }
    if reverse {
        groups.reverse();
    }
}

/// True if any socket in the group matches the search text.
pub fn group_matches(group: &Group<'_>, filter: &str) -> bool {
    group.members.iter().any(|r| matches(&r.entry, filter))
}

/// Case-insensitive match; every whitespace-separated word must appear in
/// the protocol, port, address, service name, access label ("this PC
/// only", "whole network"), PID, process name or user.
pub fn matches(entry: &PortEntry, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    let (pid, name) = match &entry.process {
        Some(p) => (p.pid.to_string(), p.name.as_str()),
        None => (String::new(), ""),
    };
    let haystack = format!(
        "{} {} {} {} {} {} {} {}",
        entry.protocol,
        entry.local.port(),
        entry.local.ip(),
        service_name(entry.protocol, entry.local.port()).unwrap_or(""),
        Access::of(entry.local.ip()).label(),
        pid,
        name,
        entry.user
    )
    .to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{ProcessInfo, Protocol};
    use std::net::SocketAddr;

    fn entry(port: u16, inode: u64, name: Option<&str>, listening: bool) -> PortEntry {
        let local: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let remote: SocketAddr = if listening {
            "0.0.0.0:0".parse().unwrap()
        } else {
            "10.0.0.2:5555".parse().unwrap()
        };
        PortEntry {
            protocol: Protocol::Tcp,
            local,
            remote,
            state: if listening { "Listen" } else { "Established" }.to_string(),
            listening,
            inode,
            user: "alice".to_string(),
            process: name.map(|n| ProcessInfo {
                pid: 100 + port as u32,
                name: n.to_string(),
                started_at: None,
            }),
        }
    }

    fn t0() -> (Instant, SystemTime) {
        (Instant::now(), SystemTime::now())
    }

    /// Sort single-socket rows through the group sorter.
    fn sort_rows(rows: &mut Vec<&Row>, key: SortKey, reverse: bool) {
        let mut groups: Vec<Group> = rows.iter().map(|r| Group { members: vec![*r] }).collect();
        sort_groups(&mut groups, key, reverse, &HashMap::new());
        *rows = groups.iter().map(|g| g.first()).collect();
    }

    fn row(e: PortEntry) -> Row {
        Row { entry: e, first_seen: None, closed_at: None }
    }

    fn at(port: u16, ip: &str, inode: u64, name: Option<&str>, pid: u32) -> PortEntry {
        let mut e = entry(port, inode, name, true);
        e.local = if ip.contains(':') {
            format!("[{ip}]:{port}").parse().unwrap()
        } else {
            format!("{ip}:{port}").parse().unwrap()
        };
        if let Some(p) = e.process.as_mut() {
            p.pid = pid;
        }
        e
    }

    #[test]
    fn ipv4_and_ipv6_sockets_of_one_program_become_one_row() {
        let a = row(at(6379, "127.0.0.1", 1, Some("redis"), 700));
        let b = row(at(6379, "::1", 2, Some("redis"), 700));
        let groups = group_rows([&a, &b]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].addresses().len(), 2);
    }

    #[test]
    fn different_programs_are_never_merged() {
        // Same UDP port, different PIDs (browser and avahi on 5353).
        let a = row(at(5353, "224.0.0.251", 1, Some("brave"), 4418));
        let b = row(at(5353, "0.0.0.0", 2, Some("avahi"), 700));
        assert_eq!(group_rows([&a, &b]).len(), 2);
        // Hidden owners merge only with the same account, same port.
        let h1 = row(at(631, "127.0.0.1", 3, None, 0));
        let h2 = row(at(631, "::1", 4, None, 0));
        let mut other = at(631, "0.0.0.0", 5, None, 0);
        other.user = "bob".into();
        let h3 = row(other);
        let groups = group_rows([&h1, &h2, &h3]);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn a_merged_row_shows_the_most_exposed_address() {
        let local = row(at(8080, "127.0.0.1", 1, Some("node"), 9));
        let open = row(at(8080, "0.0.0.0", 2, Some("node"), 9));
        let groups = group_rows([&local, &open]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].access(), Access::WholeNetwork);
    }

    #[test]
    fn a_half_closed_group_shows_only_its_open_sockets() {
        let open = row(at(6379, "127.0.0.1", 1, Some("redis"), 700));
        let mut gone = row(at(6379, "::1", 2, Some("redis"), 700));
        gone.closed_at = Some(Instant::now());
        let groups = group_rows([&open, &gone]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 1);
        assert!(!groups[0].is_closed());

        // When every member has closed, the group is closed (shown red).
        let mut gone2 = row(at(6379, "127.0.0.1", 3, Some("redis"), 700));
        gone2.closed_at = Some(Instant::now());
        let groups = group_rows([&gone, &gone2]);
        assert_eq!(groups[0].members.len(), 2);
        assert!(groups[0].is_closed());
    }

    #[test]
    fn connections_are_never_merged() {
        let mut a = entry(443, 1, Some("curl"), false);
        let mut b = entry(443, 2, Some("curl"), false);
        a.remote = "1.1.1.1:443".parse().unwrap();
        b.remote = "1.1.1.1:443".parse().unwrap();
        let (ra, rb) = (row(a), row(b));
        assert_eq!(group_rows([&ra, &rb]).len(), 2);
    }

    #[test]
    fn counts_use_merged_rows() {
        let (now, wall) = t0();
        let mut t = Tracker::default();
        t.update(
            vec![
                at(6379, "127.0.0.1", 1, Some("redis"), 700),
                at(6379, "::1", 2, Some("redis"), 700),
                at(80, "0.0.0.0", 3, None, 0),
                at(80, "::", 4, None, 0),
            ],
            now,
            wall,
        );
        assert_eq!(t.listening_count(), 2);
        assert_eq!(t.exposed_count(), 1);
        assert_eq!(t.hidden_count(), 1);
    }

    #[test]
    fn first_scan_is_not_highlighted_and_has_no_events() {
        let (now, wall) = t0();
        let mut t = Tracker::default();
        t.update(vec![entry(80, 1, Some("nginx"), true)], now, wall);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].state(now), RowState::Normal);
        assert!(t.events.is_empty());
    }

    #[test]
    fn new_port_is_green_then_fades() {
        let (now, wall) = t0();
        let mut t = Tracker::default();
        t.update(vec![], now, wall);
        t.update(vec![entry(3000, 7, Some("node"), true)], now, wall);
        assert_eq!(t.rows[0].state(now), RowState::New);
        assert_eq!(t.events.len(), 1);
        assert_eq!(t.events[0].kind, EventKind::Opened);
        assert_eq!(t.rows[0].state(now + NEW_HIGHLIGHT), RowState::Normal);
    }

    #[test]
    fn closed_port_turns_red_then_is_removed() {
        let (now, wall) = t0();
        let mut t = Tracker::default();
        t.update(vec![entry(3000, 7, Some("node"), true)], now, wall);
        t.update(vec![], now, wall);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].state(now), RowState::Closed);
        assert_eq!(t.events.back().unwrap().kind, EventKind::Closed);
        assert_eq!(t.listening_count(), 0);

        // Still there just before the keep time, gone after it.
        t.update(vec![], now + CLOSED_KEEP - Duration::from_millis(1), wall);
        assert_eq!(t.rows.len(), 1);
        t.update(vec![], now + CLOSED_KEEP, wall);
        assert!(t.rows.is_empty());
        // Only one close event, not one per scan.
        assert_eq!(t.events.len(), 1);
    }

    #[test]
    fn connections_do_not_create_events() {
        let (now, wall) = t0();
        let mut t = Tracker::default();
        t.update(vec![], now, wall);
        t.update(vec![entry(443, 9, Some("curl"), false)], now, wall);
        t.update(vec![], now, wall);
        assert!(t.events.is_empty());
    }

    #[test]
    fn owner_appearing_later_updates_the_row() {
        let (now, wall) = t0();
        let mut t = Tracker::default();
        t.update(vec![entry(80, 1, None, true)], now, wall);
        t.update(vec![entry(80, 1, Some("nginx"), true)], now, wall);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].entry.process.as_ref().unwrap().name, "nginx");
    }

    #[test]
    fn filter_matches_any_field_and_all_words() {
        let e = entry(8080, 1, Some("Python3"), true);
        assert!(matches(&e, ""));
        assert!(matches(&e, "8080"));
        assert!(matches(&e, "python"));
        assert!(matches(&e, "ALICE tcp"));
        assert!(matches(&e, "127.0.0"));
        assert!(!matches(&e, "node"));
        assert!(!matches(&e, "python udp"));
    }

    #[test]
    fn counts_exposed_and_hidden_listeners() {
        let (now, wall) = t0();
        let mut open = entry(80, 1, None, true);
        open.local = "0.0.0.0:80".parse().unwrap();
        let local = entry(5432, 2, Some("postgres"), true);
        let conn = entry(443, 3, Some("curl"), false);
        let mut t = Tracker::default();
        t.update(vec![open, local, conn], now, wall);
        assert_eq!(t.listening_count(), 2);
        assert_eq!(t.exposed_count(), 1);
        assert_eq!(t.hidden_count(), 1);
    }

    #[test]
    fn filter_understands_service_names_and_access() {
        let local = entry(5432, 1, Some("postgres"), true);
        assert!(matches(&local, "postgresql"));
        assert!(matches(&local, "this pc"));
        assert!(!matches(&local, "network"));

        let mut open = entry(80, 2, Some("nginx"), true);
        open.local = "0.0.0.0:80".parse().unwrap();
        assert!(matches(&open, "http"));
        assert!(matches(&open, "whole network"));
    }

    #[test]
    fn sorting_by_uptime_shows_newest_first_and_unknown_last() {
        let mk = |port: u16, started: Option<u64>| {
            let mut e = entry(port, port as u64, Some("p"), true);
            e.process.as_mut().unwrap().started_at = started;
            Row { entry: e, first_seen: None, closed_at: None }
        };
        let old = mk(1, Some(1_000));
        let new = mk(2, Some(9_000));
        let unknown = mk(3, None);
        let mut rows = vec![&old, &unknown, &new];
        sort_rows(&mut rows, SortKey::Uptime, false);
        let ports: Vec<_> = rows.iter().map(|r| r.entry.local.port()).collect();
        assert_eq!(ports, vec![2, 1, 3]);
    }

    #[test]
    fn sorting_by_access_puts_local_first() {
        let mk = |port: u16, ip: &str| {
            let mut e = entry(port, port as u64, Some("p"), true);
            e.local = format!("{ip}:{port}").parse().unwrap();
            Row { entry: e, first_seen: None, closed_at: None }
        };
        let open = mk(1, "0.0.0.0");
        let local = mk(2, "127.0.0.1");
        let one = mk(3, "192.168.1.5");
        let mut rows = vec![&open, &local, &one];
        sort_rows(&mut rows, SortKey::Access, false);
        let ports: Vec<_> = rows.iter().map(|r| r.entry.local.port()).collect();
        assert_eq!(ports, vec![2, 3, 1]);
    }

    #[test]
    fn sorting_puts_unknown_owners_last_and_can_reverse() {
        let a = Row { entry: entry(1, 1, Some("zeta"), true), first_seen: None, closed_at: None };
        let b = Row { entry: entry(2, 2, Some("alpha"), true), first_seen: None, closed_at: None };
        let c = Row { entry: entry(3, 3, None, true), first_seen: None, closed_at: None };
        let mut rows = vec![&a, &b, &c];

        sort_rows(&mut rows, SortKey::Process, false);
        let ports: Vec<_> = rows.iter().map(|r| r.entry.local.port()).collect();
        assert_eq!(ports, vec![2, 1, 3]);

        sort_rows(&mut rows, SortKey::Port, true);
        let ports: Vec<_> = rows.iter().map(|r| r.entry.local.port()).collect();
        assert_eq!(ports, vec![3, 2, 1]);
    }
    #[test]
    fn speed_sort_puts_the_busiest_connection_first() {
        let a = row(entry(1, 11, Some("a"), false));
        let b = row(entry(2, 22, Some("b"), false));
        let (ia, ib) = (a.entry.inode, b.entry.inode);
        let speeds = HashMap::from([
            (ia, Speed { down: 10.0, up: 0.0 }),
            (ib, Speed { down: 5.0, up: 50.0 }),
        ]);
        let mut groups: Vec<Group> = vec![Group { members: vec![&a] }, Group { members: vec![&b] }];
        sort_groups(&mut groups, SortKey::Speed, false, &speeds);
        assert_eq!(groups[0].port(), 2);
        sort_groups(&mut groups, SortKey::Speed, true, &speeds);
        assert_eq!(groups[0].port(), 1);
    }
}
