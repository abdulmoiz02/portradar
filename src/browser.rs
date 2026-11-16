//! Opening a listening port in the web browser.

use std::io;
use std::net::IpAddr;
use std::process::{Command, Stdio};

use crate::model::Group;
use crate::scanner::Protocol;

/// The URL to open for a listening TCP port, or `None` for anything that is
/// not a TCP listener (a browser cannot talk UDP).
pub fn url_for(group: &Group<'_>) -> Option<String> {
    if group.protocol() != Protocol::Tcp || !group.listening() {
        return None;
    }
    let port = group.port();

    // Prefer an address that works from this computer: wildcard or loopback.
    let ips = group.addresses();
    let host = ips
        .iter()
        .find_map(|ip| match ip {
            IpAddr::V4(v4) if v4.is_unspecified() || v4.is_loopback() => {
                Some("127.0.0.1".to_string())
            }
            IpAddr::V6(v6) if v6.is_unspecified() => Some("localhost".to_string()),
            IpAddr::V6(v6) if v6.is_loopback() => Some("[::1]".to_string()),
            _ => None,
        })
        .or_else(|| {
            ips.first().map(|ip| match ip {
                IpAddr::V4(v4) => v4.to_string(),
                IpAddr::V6(v6) => format!("[{v6}]"),
            })
        })?;

    let scheme = if matches!(port, 443 | 8443) { "https" } else { "http" };
    let default_port = matches!((scheme, port), ("http", 80) | ("https", 443));
    Some(if default_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    })
}

/// Ask the desktop to open a URL. Output is discarded so nothing is printed
/// over the dashboard, and the child is reaped on a background thread so it
/// never becomes a zombie.
pub fn open(url: &str) -> io::Result<()> {
    let mut child = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Row;
    use crate::scanner::PortEntry;

    fn row(proto: Protocol, ip: &str, port: u16) -> Row {
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
                user: "alice".into(),
                process: None,
            },
            first_seen: None,
            closed_at: None,
        }
    }

    fn url(rows: &[&Row]) -> Option<String> {
        url_for(&Group { members: rows.to_vec() })
    }

    #[test]
    fn loopback_and_wildcard_open_on_127_0_0_1() {
        assert_eq!(url(&[&row(Protocol::Tcp, "127.0.0.1", 3000)]).unwrap(), "http://127.0.0.1:3000");
        assert_eq!(url(&[&row(Protocol::Tcp, "0.0.0.0", 8080)]).unwrap(), "http://127.0.0.1:8080");
    }

    #[test]
    fn ipv6_addresses_are_bracketed() {
        assert_eq!(url(&[&row(Protocol::Tcp, "::1", 3000)]).unwrap(), "http://[::1]:3000");
        assert_eq!(url(&[&row(Protocol::Tcp, "::", 3000)]).unwrap(), "http://localhost:3000");
        assert_eq!(url(&[&row(Protocol::Tcp, "fd00::5", 3000)]).unwrap(), "http://[fd00::5]:3000");
    }

    #[test]
    fn well_known_ports_pick_the_right_scheme_and_drop_default_ports() {
        assert_eq!(url(&[&row(Protocol::Tcp, "127.0.0.1", 80)]).unwrap(), "http://127.0.0.1");
        assert_eq!(url(&[&row(Protocol::Tcp, "127.0.0.1", 443)]).unwrap(), "https://127.0.0.1");
        assert_eq!(url(&[&row(Protocol::Tcp, "127.0.0.1", 8443)]).unwrap(), "https://127.0.0.1:8443");
    }

    #[test]
    fn a_specific_network_address_is_used_when_nothing_better_exists() {
        assert_eq!(url(&[&row(Protocol::Tcp, "192.168.1.9", 3000)]).unwrap(), "http://192.168.1.9:3000");
    }

    #[test]
    fn a_mixed_group_prefers_the_local_address() {
        let a = row(Protocol::Tcp, "192.168.1.9", 3000);
        let b = row(Protocol::Tcp, "127.0.0.1", 3000);
        assert_eq!(url(&[&a, &b]).unwrap(), "http://127.0.0.1:3000");
    }

    #[test]
    fn udp_and_connections_have_no_url() {
        assert_eq!(url(&[&row(Protocol::Udp, "0.0.0.0", 53)]), None);
        let mut conn = row(Protocol::Tcp, "127.0.0.1", 3000);
        conn.entry.listening = false;
        assert_eq!(url(&[&conn]), None);
    }
}
