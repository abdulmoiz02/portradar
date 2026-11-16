//! Everything we can learn about one program and its sockets, for the
//! Inspector screen and `portwatch inspect`.
//!
//! Secrets: the program's environment variables are kept apart from
//! everything else. They are never part of the JSON, never part of the Raw
//! view, and the screen shows them masked unless you ask to reveal them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use procfs::process::Process;
use serde::Serialize;

use crate::inspect::{Hint, classify, human_duration};
use crate::scanner::{PortEntry, Protocol};

/// How many parent programs to follow up the process tree.
const MAX_PARENTS: usize = 12;

#[derive(Debug, Clone, Serialize)]
pub struct ParentFact {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessFacts {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub state: String,
    pub threads: Option<u64>,
    pub user: String,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub memory_resident_kb: Option<u64>,
    pub memory_virtual_kb: Option<u64>,
    pub cpu_seconds: Option<f64>,
    pub started_at_unix: Option<u64>,
    pub running_for_seconds: Option<u64>,
    pub open_files: Option<usize>,
    pub working_dir: Option<String>,
    pub executable: Option<String>,
    pub command_line: Vec<String>,
    pub parents: Vec<ParentFact>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SocketFacts {
    pub protocol: String,
    pub state: String,
    pub listening: bool,
    pub local: String,
    pub remote: String,
    pub inode: u64,
    pub rx_queue: Option<u32>,
    pub tx_queue: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OriginFacts {
    pub cgroup: Vec<String>,
    pub systemd_unit: Option<String>,
    pub container_id: Option<String>,
    pub docker_proxy: bool,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub pid: u32,
    pub name: String,
    pub process: Option<ProcessFacts>,
    pub sockets: Vec<SocketFacts>,
    pub origin: OriginFacts,
    /// Variable names and values. Only the Env tab ever shows these, masked
    /// unless revealed. `None` means we are not allowed to read them.
    pub env: Option<Vec<(String, String)>>,
    /// The exact text read from /proc, as (title, text) sections.
    pub raw: Vec<(String, String)>,
    /// The program has stopped since we first looked.
    pub exited: bool,
}

/// What `y` and `portwatch inspect --json` produce. No environment.
#[derive(Serialize)]
struct Report<'a> {
    portwatch_version: &'static str,
    collected_at_unix: u64,
    process: &'a Option<ProcessFacts>,
    sockets: &'a [SocketFacts],
    origin: &'a OriginFacts,
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// JSON for the Inspector's copy key and `portwatch inspect --json`.
/// It includes the command line, which can contain credentials, but never the
/// environment.
pub fn to_json(snapshot: &Snapshot) -> String {
    serde_json::to_string_pretty(&report_value(snapshot))
        .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}

pub fn report_value(snapshot: &Snapshot) -> serde_json::Value {
    let report = Report {
        portwatch_version: env!("CARGO_PKG_VERSION"),
        collected_at_unix: now_unix(),
        process: &snapshot.process,
        sockets: &snapshot.sockets,
        origin: &snapshot.origin,
    };
    serde_json::to_value(&report).unwrap_or(serde_json::Value::Null)
}

fn socket_facts(entries: &[&PortEntry]) -> Vec<SocketFacts> {
    let queues = socket_queues();
    entries
        .iter()
        .map(|e| {
            let q = queues.get(&(e.protocol, e.inode));
            SocketFacts {
                protocol: e.protocol.to_string(),
                state: e.state.clone(),
                listening: e.listening,
                local: e.local.to_string(),
                remote: e.remote.to_string(),
                inode: e.inode,
                rx_queue: q.map(|q| q.0),
                tx_queue: q.map(|q| q.1),
            }
        })
        .collect()
}

/// A snapshot for sockets whose program we are not allowed to see: only the
/// sockets themselves are known.
pub fn hidden_snapshot(entries: &[&PortEntry]) -> Snapshot {
    Snapshot {
        pid: 0,
        name: "hidden program".to_string(),
        process: None,
        sockets: socket_facts(entries),
        origin: OriginFacts {
            cgroup: Vec::new(),
            systemd_unit: None,
            container_id: None,
            docker_proxy: false,
        },
        env: None,
        raw: raw_sections_for_sockets(entries),
        exited: false,
    }
}

/// Gather a fresh snapshot for `pid`. `entries` are the sockets that belong
/// to it (as seen by the last scan). Anything we may not read is left empty.
pub fn collect(pid: u32, name: &str, entries: &[&PortEntry]) -> Snapshot {
    let proc_dir = format!("/proc/{pid}");
    let alive = std::path::Path::new(&proc_dir).exists();
    let process = Process::new(pid as i32).ok();

    let facts = process.as_ref().map(|p| process_facts(p, name));
    let env = process.as_ref().and_then(read_env);

    let cgroup_text = fs::read_to_string(format!("{proc_dir}/cgroup")).unwrap_or_default();
    let origin = origin_facts(name, &cgroup_text);

    let sockets = socket_facts(entries);
    let raw = raw_sections(pid, &cgroup_text, entries);

    Snapshot {
        pid,
        name: name.to_string(),
        process: facts,
        sockets,
        origin,
        env,
        raw,
        exited: !alive,
    }
}

fn process_facts(p: &Process, fallback_name: &str) -> ProcessFacts {
    let stat = p.stat().ok();
    let status = p.status().ok();

    let ticks = procfs::ticks_per_second().max(1);
    let started_at = stat
        .as_ref()
        .zip(procfs::boot_time_secs().ok())
        .map(|(s, boot)| boot + s.starttime / ticks);
    let running_for = started_at.map(|s| now_unix().saturating_sub(s));
    let uid = status.as_ref().map(|s| s.ruid);

    ProcessFacts {
        pid: p.pid as u32,
        ppid: stat.as_ref().map(|s| s.ppid as u32),
        name: stat
            .as_ref()
            .map(|s| s.comm.clone())
            .unwrap_or_else(|| fallback_name.to_string()),
        state: stat.as_ref().map(|s| describe_state(s.state)).unwrap_or_default(),
        threads: status.as_ref().map(|s| s.threads),
        user: uid.map(user_name).unwrap_or_default(),
        uid,
        gid: status.as_ref().map(|s| s.rgid),
        memory_resident_kb: status.as_ref().and_then(|s| s.vmrss),
        memory_virtual_kb: status.as_ref().and_then(|s| s.vmsize),
        cpu_seconds: stat.as_ref().map(|s| (s.utime + s.stime) as f64 / ticks as f64),
        started_at_unix: started_at,
        running_for_seconds: running_for,
        open_files: p.fd_count().ok(),
        working_dir: p.cwd().ok().map(|d| d.display().to_string()),
        executable: p.exe().ok().map(|d| d.display().to_string()),
        command_line: p.cmdline().unwrap_or_default(),
        parents: parent_chain(stat.as_ref().map(|s| s.ppid as u32)),
    }
}

fn user_name(uid: u32) -> String {
    nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_else(|| uid.to_string())
}

pub fn describe_state(code: char) -> String {
    let meaning = match code {
        'R' => "running",
        'S' => "sleeping",
        'D' => "waiting on disk",
        'Z' => "zombie (finished, not yet cleaned up)",
        'T' | 't' => "stopped",
        'I' => "idle",
        'X' | 'x' => "dead",
        _ => "unknown",
    };
    format!("{code} ({meaning})")
}

/// Follow parent PIDs up the tree. Capped, stops at PID 0 or 1 (after
/// listing 1), and never loops.
fn parent_chain(first: Option<u32>) -> Vec<ParentFact> {
    let mut chain = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut current = first;
    while let Some(pid) = current {
        if pid == 0 || chain.len() >= MAX_PARENTS || !seen.insert(pid) {
            break;
        }
        let Ok(parent) = Process::new(pid as i32) else { break };
        let Ok(stat) = parent.stat() else { break };
        chain.push(ParentFact { pid, name: stat.comm.clone() });
        if pid == 1 {
            break;
        }
        current = Some(stat.ppid as u32);
    }
    chain
}

fn read_env(p: &Process) -> Option<Vec<(String, String)>> {
    let vars = p.environ().ok()?;
    let mut list: Vec<(String, String)> = vars
        .into_iter()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .collect();
    list.sort();
    Some(list)
}

fn origin_facts(name: &str, cgroup: &str) -> OriginFacts {
    let hint = classify(name, cgroup);
    OriginFacts {
        cgroup: cgroup
            .lines()
            .map(|l| l.rsplit(':').next().unwrap_or(l).to_string())
            .collect(),
        systemd_unit: match &hint {
            Some(Hint::Systemd(unit)) => Some(unit.clone()),
            _ => None,
        },
        container_id: match &hint {
            Some(Hint::Container(id)) => Some(id.clone()),
            _ => None,
        },
        docker_proxy: matches!(hint, Some(Hint::DockerProxy)),
    }
}

/// (protocol, inode) -> (rx queue, tx queue), read fresh from the kernel.
fn socket_queues() -> HashMap<(Protocol, u64), (u32, u32)> {
    let mut map = HashMap::new();
    for e in procfs::net::tcp().unwrap_or_default().into_iter().chain(procfs::net::tcp6().unwrap_or_default()) {
        map.insert((Protocol::Tcp, e.inode), (e.rx_queue, e.tx_queue));
    }
    for e in procfs::net::udp().unwrap_or_default().into_iter().chain(procfs::net::udp6().unwrap_or_default()) {
        map.insert((Protocol::Udp, e.inode), (e.rx_queue, e.tx_queue));
    }
    map
}

/// The raw text portwatch based its answers on. Never includes the
/// environment.
fn raw_sections(pid: u32, cgroup_text: &str, entries: &[&PortEntry]) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    for file in ["status", "stat"] {
        if let Ok(text) = fs::read_to_string(format!("/proc/{pid}/{file}")) {
            sections.push((format!("/proc/{pid}/{file}"), text));
        }
    }
    if !cgroup_text.is_empty() {
        sections.push((format!("/proc/{pid}/cgroup"), cgroup_text.to_string()));
    }
    sections.extend(raw_sections_for_sockets(entries));
    sections
}

/// The rows of `/proc/net/{tcp,tcp6,udp,udp6}` that describe these sockets.
fn raw_sections_for_sockets(entries: &[&PortEntry]) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let inodes: HashSet<u64> = entries.iter().map(|e| e.inode).collect();
    for table in ["tcp", "tcp6", "udp", "udp6"] {
        let Ok(text) = fs::read_to_string(format!("/proc/net/{table}")) else { continue };
        let mut lines = text.lines();
        let Some(header) = lines.next() else { continue };
        let rows: Vec<&str> = lines
            .filter(|l| {
                l.split_whitespace()
                    .nth(9)
                    .and_then(|n| n.parse::<u64>().ok())
                    .is_some_and(|inode| inodes.contains(&inode))
            })
            .collect();
        if !rows.is_empty() {
            sections.push((
                format!("/proc/net/{table} (this program's rows)"),
                format!("{header}\n{}\n", rows.join("\n")),
            ));
        }
    }
    sections
}

// ---- plain text, shared by the Inspector and `portwatch inspect` ---------

pub fn human_kb(kb: u64) -> String {
    const MB: u64 = 1024;
    const GB: u64 = 1024 * 1024;
    if kb >= GB {
        format!("{:.1} GB", kb as f64 / GB as f64)
    } else if kb >= MB {
        format!("{} MB", kb / MB)
    } else {
        format!("{kb} kB")
    }
}

/// Label/value pairs describing the process, in reading order.
pub fn process_rows(p: &ProcessFacts) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Name", p.name.clone())];
    rows.push((
        "PID / parent",
        match p.ppid {
            Some(ppid) => format!("{} / {ppid}", p.pid),
            None => p.pid.to_string(),
        },
    ));
    if !p.state.is_empty() {
        rows.push(("State", p.state.clone()));
    }
    if !p.user.is_empty() {
        rows.push((
            "User",
            match (p.uid, p.gid) {
                (Some(u), Some(g)) => format!("{} (uid {u}, gid {g})", p.user),
                _ => p.user.clone(),
            },
        ));
    }
    if let Some(t) = p.threads {
        rows.push(("Threads", t.to_string()));
    }
    if let Some(rss) = p.memory_resident_kb {
        let virt = p
            .memory_virtual_kb
            .map(|v| format!(", virtual {}", human_kb(v)))
            .unwrap_or_default();
        rows.push(("Memory", format!("in use {}{virt}", human_kb(rss))));
    }
    if let Some(cpu) = p.cpu_seconds {
        rows.push(("CPU time", format!("{cpu:.1} s in total")));
    }
    if let Some(started) = p.started_at_unix {
        let when = chrono::DateTime::from_timestamp(started as i64, 0)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        let ago = p
            .running_for_seconds
            .map(|s| format!(" ({} ago)", human_duration(s)))
            .unwrap_or_default();
        rows.push(("Started", format!("{when}{ago}")));
    }
    if let Some(n) = p.open_files {
        rows.push(("Open files", n.to_string()));
    }
    if let Some(dir) = &p.working_dir {
        rows.push(("Folder", dir.clone()));
    }
    if let Some(exe) = &p.executable {
        rows.push(("Program file", exe.clone()));
    }
    if !p.parents.is_empty() {
        let chain: Vec<String> = p.parents.iter().map(|x| format!("{} {}", x.pid, x.name)).collect();
        rows.push(("Started by", chain.join(" <- ")));
    }
    rows
}

/// One line per socket: `tcp  Established 127.0.0.1:3000  10.0.0.2:5555  rx0 tx0 inode 123`.
pub fn socket_line(s: &SocketFacts, host: Option<&str>) -> String {
    let queues = match (s.rx_queue, s.tx_queue) {
        (Some(rx), Some(tx)) => format!("rx{rx} tx{tx}"),
        _ => "rx- tx-".to_string(),
    };
    let remote = if s.listening { "-".to_string() } else { s.remote.clone() };
    let host = host.map(|h| format!("  ({h})")).unwrap_or_default();
    format!(
        "{:<5} {:<11} {:<21} {:<21} {queues} inode {}{host}",
        s.protocol, s.state, s.local, remote, s.inode
    )
}

/// Plain-text report for `portwatch inspect <port>` (no colours, no screen).
pub fn report_text(snapshot: &Snapshot) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} (PID {})\n", snapshot.name, snapshot.pid));
    match &snapshot.process {
        Some(p) => {
            for (label, value) in process_rows(p) {
                out.push_str(&format!("  {label:<13}{value}\n"));
            }
            for (i, arg) in p.command_line.iter().enumerate() {
                out.push_str(&format!("  {:<13}{arg}\n", if i == 0 { "Command" } else { "" }));
            }
        }
        None => out.push_str("  (details are not readable: the program belongs to another user)\n"),
    }
    out.push_str("Sockets\n");
    for s in &snapshot.sockets {
        out.push_str(&format!("  {}\n", socket_line(s, None)));
    }
    if let Some(unit) = &snapshot.origin.systemd_unit {
        out.push_str(&format!("Managed by systemd: {unit}\n"));
    }
    if let Some(id) = &snapshot.origin.container_id {
        out.push_str(&format!("Runs in container: {id}\n"));
    }
    if snapshot.origin.docker_proxy {
        out.push_str("This is docker-proxy: it forwards a published container port\n");
    }
    out
}

const INSPECT_USAGE: &str = "\
USAGE:
    portwatch inspect <port> [--json]

Shows everything portwatch can learn about the program using a port: its
process details, every socket it has, and where it comes from (systemd or
Docker). With --json it prints machine-readable JSON (no environment
variables) that you can pipe anywhere. The command line is included, and
command lines can contain passwords.";

/// `portwatch inspect <port> [--json]`.
pub fn run_cli(args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;

    let mut port: Option<u16> = None;
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{INSPECT_USAGE}");
                return ExitCode::SUCCESS;
            }
            other => match other.parse::<u16>() {
                Ok(p) if p > 0 && port.is_none() => port = Some(p),
                _ => {
                    eprintln!("portwatch inspect: unexpected argument '{other}'\n\n{INSPECT_USAGE}");
                    return ExitCode::from(2);
                }
            },
        }
    }
    let Some(port) = port else {
        eprintln!("portwatch inspect: which port?\n\n{INSPECT_USAGE}");
        return ExitCode::from(2);
    };

    let entries = match crate::scanner::scan() {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("portwatch inspect: could not read /proc: {err}");
            return ExitCode::FAILURE;
        }
    };
    let snapshots = snapshots_for_port(&entries, port);
    if snapshots.is_empty() {
        eprintln!("Nothing is listening on port {port}.");
        return ExitCode::FAILURE;
    }

    if json {
        let values: Vec<serde_json::Value> = snapshots.iter().map(report_value).collect();
        let value = if values.len() == 1 {
            values.into_iter().next().unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Array(values)
        };
        println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
    } else {
        for snapshot in &snapshots {
            print!("{}", report_text(snapshot));
        }
    }
    ExitCode::SUCCESS
}

/// One snapshot per program listening on `port`, plus one for hidden owners.
/// Each program's snapshot lists all of its sockets, not only this port.
pub fn snapshots_for_port(entries: &[PortEntry], port: u16) -> Vec<Snapshot> {
    let listeners: Vec<&PortEntry> = entries
        .iter()
        .filter(|e| e.listening && e.local.port() == port)
        .collect();

    let mut holders: Vec<(u32, String)> = listeners
        .iter()
        .filter_map(|e| e.process.as_ref())
        .map(|p| (p.pid, p.name.clone()))
        .collect();
    holders.sort();
    holders.dedup();

    let mut snapshots: Vec<Snapshot> = holders
        .into_iter()
        .map(|(pid, name)| {
            let own: Vec<&PortEntry> = entries
                .iter()
                .filter(|e| e.process.as_ref().is_some_and(|p| p.pid == pid))
                .collect();
            collect(pid, &name, &own)
        })
        .collect();

    let hidden: Vec<&PortEntry> = listeners.iter().copied().filter(|e| e.process.is_none()).collect();
    if !hidden.is_empty() {
        snapshots.push(hidden_snapshot(&hidden));
    }
    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::scan;
    use std::net::TcpListener;

    /// Our own process, with a real listening socket.
    fn own_snapshot() -> (Snapshot, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let entries = scan().unwrap();
        let mine: Vec<&PortEntry> = entries
            .iter()
            .filter(|e| e.process.as_ref().is_some_and(|p| p.pid == std::process::id()))
            .collect();
        assert!(!mine.is_empty());
        (collect(std::process::id(), "portwatch", &mine), listener)
    }

    #[test]
    fn describes_our_own_process() {
        let (snap, _keep) = own_snapshot();
        let p = snap.process.as_ref().expect("we can read ourselves");
        assert_eq!(p.pid, std::process::id());
        assert!(p.ppid.is_some());
        assert!(!p.command_line.is_empty());
        assert!(p.threads.unwrap_or(0) >= 1);
        assert!(p.memory_resident_kb.unwrap_or(0) > 0);
        assert!(p.open_files.unwrap_or(0) > 0);
        assert!(p.running_for_seconds.is_some());
        assert!(!snap.exited);
        // The test's own listener is listed with its queues.
        assert!(snap.sockets.iter().any(|s| s.listening && s.rx_queue.is_some()));
    }

    #[test]
    fn the_parent_chain_ends_at_init_without_looping() {
        let (snap, _keep) = own_snapshot();
        let chain = &snap.process.unwrap().parents;
        assert!(!chain.is_empty() && chain.len() <= MAX_PARENTS);
        let pids: HashSet<u32> = chain.iter().map(|p| p.pid).collect();
        assert_eq!(pids.len(), chain.len(), "no repeats");
        assert!(chain.iter().all(|p| p.pid != 0));
        // An invalid starting point gives an empty chain instead of a crash.
        assert!(parent_chain(Some(0)).is_empty());
        assert!(parent_chain(None).is_empty());
        assert!(parent_chain(Some(4_000_000)).is_empty());
    }

    #[test]
    fn the_environment_is_never_in_the_json_or_the_raw_sections() {
        // A child with a secret in its environment.
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .env("PORTWATCH_TEST_SECRET", "hunter2-super-secret")
            .spawn()
            .unwrap();
        for _ in 0..300 {
            let comm = fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap_or_default();
            let env = fs::read(format!("/proc/{}/environ", child.id())).unwrap_or_default();
            if comm.trim_end() == "sleep" && !env.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let snap = collect(child.id(), "sleep", &[]);

        // We can read our own child's environment, so the value is held for
        // the Env tab...
        let env = snap.env.as_ref().expect("same user can read the environment");
        assert!(env.iter().any(|(k, v)| k == "PORTWATCH_TEST_SECRET" && v == "hunter2-super-secret"));

        // ...but it must not appear anywhere else.
        let json = to_json(&snap);
        assert!(!json.contains("hunter2"), "secret leaked into the JSON");
        assert!(!json.contains("PORTWATCH_TEST_SECRET"), "variable names leaked into the JSON");
        let raw: String = snap.raw.iter().map(|(t, s)| format!("{t}\n{s}")).collect();
        assert!(!raw.contains("hunter2"), "secret leaked into the raw view");
        assert!(!raw.contains("environ"), "environ is not a raw section");
        let text = report_text(&snap);
        assert!(!text.contains("hunter2"), "secret leaked into the text report");

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn json_is_valid_and_has_the_expected_shape() {
        let (snap, _keep) = own_snapshot();
        let value: serde_json::Value = serde_json::from_str(&to_json(&snap)).unwrap();
        assert_eq!(value["process"]["pid"], std::process::id());
        assert!(value["sockets"].as_array().is_some_and(|a| !a.is_empty()));
        assert!(value["origin"]["cgroup"].is_array());
        assert!(value["collected_at_unix"].as_u64().unwrap() > 1_700_000_000);
        assert!(value.get("env").is_none());
    }

    #[test]
    fn a_program_that_is_gone_is_reported_not_fatal() {
        let snap = collect(4_000_000, "ghost", &[]);
        assert!(snap.exited);
        assert!(snap.process.is_none());
        assert!(snap.env.is_none());
        assert_eq!(snap.name, "ghost");
        let json = to_json(&snap);
        assert!(json.contains("\"process\": null"));
    }

    #[test]
    fn raw_sections_show_the_proc_text_that_was_used() {
        let (snap, _keep) = own_snapshot();
        let titles: Vec<&str> = snap.raw.iter().map(|(t, _)| t.as_str()).collect();
        assert!(titles.iter().any(|t| t.ends_with("/status")), "{titles:?}");
        assert!(titles.iter().any(|t| t.ends_with("/stat")), "{titles:?}");
        assert!(titles.iter().any(|t| t.starts_with("/proc/net/tcp")), "{titles:?}");
    }

    #[test]
    fn states_and_sizes_read_naturally() {
        assert_eq!(describe_state('S'), "S (sleeping)");
        assert!(describe_state('Z').contains("zombie"));
        assert_eq!(describe_state('?'), "? (unknown)");
        assert_eq!(human_kb(512), "512 kB");
        assert_eq!(human_kb(85 * 1024), "85 MB");
        assert_eq!(human_kb(3 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn socket_lines_include_queues_and_optional_host() {
        let s = SocketFacts {
            protocol: "tcp".into(),
            state: "Established".into(),
            listening: false,
            local: "127.0.0.1:3000".into(),
            remote: "10.0.0.2:5555".into(),
            inode: 42,
            rx_queue: Some(1),
            tx_queue: Some(2),
        };
        let line = socket_line(&s, Some("example.test"));
        assert!(line.contains("Established") && line.contains("rx1 tx2") && line.contains("inode 42"));
        assert!(line.ends_with("(example.test)"));
        assert!(socket_line(&s, None).contains("10.0.0.2:5555"));
    }

    #[test]
    fn the_plain_report_covers_process_and_sockets() {
        let (snap, _keep) = own_snapshot();
        let text = report_text(&snap);
        assert!(text.contains("Sockets") && text.contains("Started by") && text.contains("Command"));
    }
}
