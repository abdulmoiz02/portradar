//! A quick, plain-English opinion on each row.
//!
//! These are simple rules, not a security scanner. Each verdict carries the
//! reason it was given, so you can judge for yourself.

use crate::model::Group;
use crate::scanner::Protocol;
use crate::services::{Access, service_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Nothing unusual.
    Normal,
    /// A development server you started.
    Dev,
    /// Worth a second look.
    Check,
    /// No opinion either way.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub level: Level,
    pub reason: &'static str,
}

const DATABASES: &[&str] = &[
    "MySQL",
    "PostgreSQL",
    "Redis",
    "MongoDB",
    "Elasticsearch",
    "Memcached",
];

const DEV_SERVICES: &[&str] = &["Dev server", "Vite dev", "Angular dev", "Web (alt)"];

const DEV_PROGRAMS: &[&str] = &[
    "node", "nodejs", "python", "python3", "ruby", "php", "deno", "bun", "vite", "next-server",
    "ts-node", "uvicorn", "gunicorn", "flask", "rails", "puma", "webpack",
];

/// Services that are normal for an operating system to expose.
const EXPECTED_WHEN_OPEN: &[&str] = &[
    "DNS",
    "DHCP",
    "mDNS discovery",
    "LLMNR lookup",
    "Device discovery",
    "Time sync (NTP)",
    "Web (HTTP)",
    "Web (HTTPS)",
    "SSH",
    "Printing (CUPS)",
];

const NOISE_SERVICES: &[&str] = &[
    "DNS",
    "DHCP",
    "mDNS discovery",
    "LLMNR lookup",
    "Device discovery",
    "Time sync (NTP)",
    "Printing (CUPS)",
];

const NOISE_PROGRAMS: &[&str] = &[
    "avahi-daemon",
    "systemd-resolve",
    "systemd-resolved",
    "cupsd",
    "cups-browsed",
    "chronyd",
    "NetworkManager",
    "dhclient",
];

const NOISE_ACCOUNTS: &[&str] = &[
    "avahi",
    "avahi-autoipd",
    "systemd-resolve",
    "systemd-network",
    "systemd-timesync",
    "colord",
    "polkitd",
    "messagebus",
    "rtkit",
];

pub fn verdict(group: &Group<'_>, me: &str) -> Verdict {
    let access = group.access();
    let exposed = access.is_exposed();
    let service = service_name(group.protocol(), group.port());
    let program = group.process().map(|p| p.name.as_str());
    let mine = group.user() == me;
    let is_dev = mine
        && group.protocol() == Protocol::Tcp
        && (service.is_some_and(|s| DEV_SERVICES.contains(&s))
            || program.is_some_and(|p| DEV_PROGRAMS.contains(&p)));

    if exposed {
        if service.is_some_and(|s| DATABASES.contains(&s)) {
            return Verdict {
                level: Level::Check,
                reason: "A database is open to the network. Databases are normally kept to this computer only.",
            };
        }
        if matches!(service, Some("Telnet") | Some("FTP")) {
            return Verdict {
                level: Level::Check,
                reason: "Telnet and FTP send passwords in plain text. Avoid exposing them.",
            };
        }
        if is_dev {
            return Verdict {
                level: Level::Check,
                reason: "Your development server can be reached by other devices on your network.",
            };
        }
        if program.is_none() && service.is_none() {
            return Verdict {
                level: Level::Check,
                reason: "Open to the network, and nothing here says what it is. Run with sudo to see the program.",
            };
        }
        if service.is_some_and(|s| EXPECTED_WHEN_OPEN.contains(&s)) {
            return Verdict {
                level: Level::Normal,
                reason: "A common service that is expected to be reachable.",
            };
        }
        return Verdict {
            level: Level::Unknown,
            reason: "Open to the network. Nothing looks wrong, but nothing confirms what it is either.",
        };
    }

    if is_dev {
        return Verdict {
            level: Level::Dev,
            reason: "A development server you started, reachable only from this computer.",
        };
    }
    Verdict {
        level: Level::Normal,
        reason: "Nothing unusual about it.",
    }
}

/// System plumbing that most people do not need to look at (DNS, printing,
/// discovery, time sync). Used by the "hide system noise" filter. Databases
/// and web servers are never treated as noise.
pub fn is_noise(group: &Group<'_>) -> bool {
    if service_name(group.protocol(), group.port()).is_some_and(|s| NOISE_SERVICES.contains(&s)) {
        return true;
    }
    match group.process() {
        Some(p) => NOISE_PROGRAMS.contains(&p.name.as_str()),
        None => NOISE_ACCOUNTS.contains(&group.user()),
    }
}

/// Whether a sentence about exposure should mention that a second address
/// of the same program is local-only.
pub fn exposure_note(group: &Group<'_>) -> Option<&'static str> {
    let ips = group.addresses();
    let has_local = ips.iter().any(|ip| Access::of(*ip) == Access::ThisPcOnly);
    if group.access().is_exposed() && has_local {
        Some("It also listens on this computer's own address.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Group, Row};
    use crate::scanner::{PortEntry, ProcessInfo};

    fn row(proto: Protocol, ip: &str, port: u16, program: Option<&str>, user: &str) -> Row {
        let local = if ip.contains(':') {
            format!("[{ip}]:{port}")
        } else {
            format!("{ip}:{port}")
        };
        Row {
            entry: PortEntry {
                protocol: proto,
                local: local.parse().unwrap(),
                remote: "0.0.0.0:0".parse().unwrap(),
                state: "Listen".into(),
                listening: true,
                inode: port as u64,
                user: user.into(),
                process: program.map(|n| ProcessInfo {
                    pid: 100,
                    name: n.into(),
                    started_at: None,
                }),
            },
            first_seen: None,
            closed_at: None,
        }
    }

    fn level(r: &Row, me: &str) -> Level {
        verdict(&Group { members: vec![r] }, me).level
    }

    #[test]
    fn local_only_things_are_normal() {
        let r = row(Protocol::Tcp, "127.0.0.1", 5432, Some("postgres"), "postgres");
        assert_eq!(level(&r, "alice"), Level::Normal);
    }

    #[test]
    fn your_own_dev_server_is_a_dev_server_until_it_is_exposed() {
        let local = row(Protocol::Tcp, "127.0.0.1", 3000, Some("node"), "alice");
        assert_eq!(level(&local, "alice"), Level::Dev);
        let open = row(Protocol::Tcp, "0.0.0.0", 3000, Some("node"), "alice");
        assert_eq!(level(&open, "alice"), Level::Check);
        // Someone else's node process is not "your" dev server.
        assert_eq!(level(&local, "bob"), Level::Normal);
    }

    #[test]
    fn exposed_databases_and_telnet_are_flagged() {
        let db = row(Protocol::Tcp, "0.0.0.0", 6379, Some("redis-server"), "redis");
        assert_eq!(level(&db, "alice"), Level::Check);
        let telnet = row(Protocol::Tcp, "::", 23, Some("telnetd"), "root");
        assert_eq!(level(&telnet, "alice"), Level::Check);
    }

    #[test]
    fn unknown_exposed_ports_with_hidden_owners_are_flagged() {
        let r = row(Protocol::Udp, "0.0.0.0", 53175, None, "avahi");
        assert_eq!(level(&r, "alice"), Level::Check);
    }

    #[test]
    fn expected_services_are_normal_even_when_open() {
        let ssh = row(Protocol::Tcp, "0.0.0.0", 22, None, "root");
        assert_eq!(level(&ssh, "alice"), Level::Normal);
        let mdns = row(Protocol::Udp, "0.0.0.0", 5353, None, "avahi");
        assert_eq!(level(&mdns, "alice"), Level::Normal);
    }

    #[test]
    fn exposed_but_unrecognised_gets_no_opinion() {
        let r = row(Protocol::Tcp, "0.0.0.0", 40000, Some("spotify"), "alice");
        assert_eq!(level(&r, "alice"), Level::Unknown);
    }

    #[test]
    fn noise_covers_plumbing_but_never_databases_or_web_servers() {
        let noise = |r: &Row| is_noise(&Group { members: vec![r] });
        assert!(noise(&row(Protocol::Tcp, "127.0.0.53", 53, None, "systemd-resolve")));
        assert!(noise(&row(Protocol::Udp, "0.0.0.0", 5353, Some("brave"), "alice")));
        assert!(noise(&row(Protocol::Udp, "0.0.0.0", 47040, None, "avahi")));
        assert!(!noise(&row(Protocol::Tcp, "127.0.0.1", 6379, None, "redis")));
        assert!(!noise(&row(Protocol::Tcp, "0.0.0.0", 80, None, "root")));
        assert!(!noise(&row(Protocol::Tcp, "127.0.0.1", 3000, Some("node"), "alice")));
    }
}
