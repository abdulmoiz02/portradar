//! `portradar free <port>`: find what is holding a port, stop it (after
//! asking), and check that the port really became free.
//!
//! Unlike `fuser -k`, this asks first, sends SIGTERM before anything harsher,
//! refuses to touch PID 1 or itself, and reports whether the port was freed.

use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::inspect::{human_duration, inspect};
use crate::kill;
use crate::scanner::{PortEntry, Protocol, scan};

/// How long to wait for the port to be released after signalling.
pub const WAIT_FOR_RELEASE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Holder {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Nothing is listening on the port.
    Free,
    /// Programs we can see are listening on it. `hidden_users` lists
    /// accounts running further listeners we cannot see.
    Held {
        holders: Vec<Holder>,
        hidden_users: Vec<String>,
    },
    /// Something listens, but every owner is hidden from us.
    OnlyHidden { users: Vec<String> },
}

/// Who is listening on `port` (any protocol), from a scan result.
pub fn lookup(entries: &[PortEntry], port: u16) -> Lookup {
    let listeners: Vec<&PortEntry> = entries
        .iter()
        .filter(|e| e.listening && e.local.port() == port)
        .collect();
    if listeners.is_empty() {
        return Lookup::Free;
    }

    let mut holders: Vec<Holder> = listeners
        .iter()
        .filter_map(|e| e.process.as_ref())
        .map(|p| Holder { pid: p.pid, name: p.name.clone() })
        .collect();
    holders.sort();
    holders.dedup();

    let mut hidden_users: Vec<String> = listeners
        .iter()
        .filter(|e| e.process.is_none())
        .map(|e| e.user.clone())
        .collect();
    hidden_users.sort();
    hidden_users.dedup();

    if holders.is_empty() {
        Lookup::OnlyHidden { users: hidden_users }
    } else {
        Lookup::Held { holders, hidden_users }
    }
}

/// Poll until nothing listens on `port` or `timeout` passes. Returns the
/// final lookup.
pub fn wait_until_free(port: u16, timeout: Duration) -> io::Result<Lookup> {
    let deadline = Instant::now() + timeout;
    loop {
        let entries = scan().map_err(|e| io::Error::other(e.to_string()))?;
        let result = lookup(&entries, port);
        if result == Lookup::Free || Instant::now() >= deadline {
            return Ok(result);
        }
        sleep(Duration::from_millis(100));
    }
}

pub fn protocols_label(entries: &[PortEntry], port: u16) -> String {
    let mut names: Vec<String> = entries
        .iter()
        .filter(|e| e.listening && e.local.port() == port)
        .map(|e| match e.protocol {
            Protocol::Tcp => "tcp".to_string(),
            Protocol::Udp => "udp".to_string(),
        })
        .collect();
    names.sort();
    names.dedup();
    names.join("+")
}

const USAGE: &str = "\
USAGE:
    portradar free <port> [--yes] [--force]

Finds what is using the port, asks before stopping it, then checks that the
port is really free.

    --yes, -y    do not ask for confirmation (needed when not in a terminal)
    --force      send SIGKILL instead of the polite SIGTERM";

pub fn run(args: &[String]) -> ExitCode {
    let mut port: Option<u16> = None;
    let mut yes = false;
    let mut force = false;
    for arg in args {
        match arg.as_str() {
            "--yes" | "-y" => yes = true,
            "--force" => force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => match other.parse::<u16>() {
                Ok(p) if p > 0 && port.is_none() => port = Some(p),
                _ => {
                    eprintln!("portradar free: unexpected argument '{other}'\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
        }
    }
    let Some(port) = port else {
        eprintln!("portradar free: which port?\n\n{USAGE}");
        return ExitCode::from(2);
    };

    match free_port(port, yes, force) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("portradar free: {err}");
            ExitCode::FAILURE
        }
    }
}

fn free_port(port: u16, yes: bool, force: bool) -> io::Result<ExitCode> {
    let entries = scan().map_err(|e| io::Error::other(format!("could not read /proc: {e}")))?;
    let (holders, hidden_users) = match lookup(&entries, port) {
        Lookup::Free => {
            println!("Port {port} is already free.");
            return Ok(ExitCode::SUCCESS);
        }
        Lookup::OnlyHidden { users } => {
            eprintln!(
                "Port {port} is in use by a program owned by {} that you cannot see.\nRun it with sudo to manage it: sudo \"$(which portradar)\" free {port}",
                users.join(", ")
            );
            return Ok(ExitCode::FAILURE);
        }
        Lookup::Held { holders, hidden_users } => (holders, hidden_users),
    };

    println!("Port {port}/{} is in use by:", protocols_label(&entries, port));
    for holder in &holders {
        let details = inspect(holder.pid, &holder.name);
        let age = details
            .running_for_secs
            .map(|s| format!(", running for {}", human_duration(s)))
            .unwrap_or_default();
        println!("  {} (PID {}){age}", holder.name, holder.pid);
        if !details.cmdline.is_empty() {
            println!("    $ {}", details.cmdline);
        }
        if let Some(hint) = details.hint {
            println!("    ! {}", hint.advice());
        }
    }
    if !hidden_users.is_empty() {
        println!(
            "  and a program owned by {} that you cannot see (run with sudo to include it)",
            hidden_users.join(", ")
        );
    }

    if !yes {
        if !io::stdin().is_terminal() {
            eprintln!("Not in a terminal, so I cannot ask. Add --yes to stop it anyway.");
            return Ok(ExitCode::from(2));
        }
        let how = if force { "SIGKILL (immediate)" } else { "SIGTERM (polite)" };
        print!("Stop the program above with {how}? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("Cancelled. Nothing was stopped.");
            return Ok(ExitCode::SUCCESS);
        }
    }

    for holder in &holders {
        match kill::stop(holder.pid, &holder.name, force) {
            Ok(()) => println!(
                "Sent {} to {} (PID {}).",
                if force { "SIGKILL" } else { "SIGTERM" },
                holder.name,
                holder.pid
            ),
            Err(err) => eprintln!("Could not stop {} (PID {}): {err}", holder.name, holder.pid),
        }
    }

    // Success means the port is actually free, whatever the signals said.
    match wait_until_free(port, WAIT_FOR_RELEASE)? {
        Lookup::Free => {
            println!("Port {port} is free.");
            Ok(ExitCode::SUCCESS)
        }
        Lookup::Held { holders, .. } => {
            let names: Vec<String> = holders
                .iter()
                .map(|h| format!("{} (PID {})", h.name, h.pid))
                .collect();
            eprintln!(
                "Port {port} is still in use by {}.{}",
                names.join(", "),
                if force { "" } else { " Try again with --force." }
            );
            Ok(ExitCode::FAILURE)
        }
        Lookup::OnlyHidden { users } => {
            eprintln!(
                "Port {port} is still in use by a program owned by {} that you cannot see.",
                users.join(", ")
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::ProcessInfo;

    fn listener(proto: Protocol, port: u16, program: Option<(&str, u32)>, user: &str) -> PortEntry {
        PortEntry {
            protocol: proto,
            local: format!("127.0.0.1:{port}").parse().unwrap(),
            remote: "0.0.0.0:0".parse().unwrap(),
            state: "Listen".into(),
            listening: true,
            inode: port as u64,
            user: user.into(),
            process: program.map(|(n, pid)| ProcessInfo {
                pid,
                name: n.into(),
                started_at: None,
            }),
        }
    }

    #[test]
    fn an_unused_port_is_free() {
        let entries = vec![listener(Protocol::Tcp, 80, Some(("nginx", 5)), "root")];
        assert_eq!(lookup(&entries, 3000), Lookup::Free);
    }

    #[test]
    fn connections_do_not_count_as_holding_a_port() {
        let mut conn = listener(Protocol::Tcp, 3000, Some(("curl", 9)), "alice");
        conn.listening = false;
        assert_eq!(lookup(&[conn], 3000), Lookup::Free);
    }

    #[test]
    fn the_same_program_on_ipv4_and_ipv6_is_one_holder() {
        let mut v6 = listener(Protocol::Tcp, 3000, Some(("node", 42)), "alice");
        v6.local = "[::1]:3000".parse().unwrap();
        let entries = vec![listener(Protocol::Tcp, 3000, Some(("node", 42)), "alice"), v6];
        match lookup(&entries, 3000) {
            Lookup::Held { holders, hidden_users } => {
                assert_eq!(holders, vec![Holder { pid: 42, name: "node".into() }]);
                assert!(hidden_users.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn several_programs_are_all_listed_and_hidden_ones_are_reported() {
        let entries = vec![
            listener(Protocol::Tcp, 8080, Some(("worker", 11)), "alice"),
            listener(Protocol::Tcp, 8080, Some(("worker", 12)), "alice"),
            listener(Protocol::Tcp, 8080, None, "root"),
        ];
        match lookup(&entries, 8080) {
            Lookup::Held { holders, hidden_users } => {
                assert_eq!(holders.len(), 2);
                assert_eq!(hidden_users, vec!["root".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn only_hidden_owners_is_reported_separately() {
        let entries = vec![listener(Protocol::Tcp, 631, None, "root")];
        assert_eq!(
            lookup(&entries, 631),
            Lookup::OnlyHidden { users: vec!["root".to_string()] }
        );
    }

    #[test]
    fn usage_errors_exit_with_code_2() {
        // ExitCode has no PartialEq, so compare how it prints.
        let two = format!("{:?}", ExitCode::from(2));
        let code = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            format!("{:?}", run(&args))
        };
        assert_eq!(code(&[]), two);
        assert_eq!(code(&["abc"]), two);
        assert_eq!(code(&["0"]), two);
        assert_eq!(code(&["80", "81"]), two);
    }
}
