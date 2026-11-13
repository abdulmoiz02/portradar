//! Reads the kernel's socket tables from `/proc` and maps each socket to the
//! process that owns it.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;

use procfs::net::{TcpState, tcp, tcp6, udp, udp6};
use procfs::process::{FDTarget, all_processes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Protocol::Tcp => f.write_str("tcp"),
            Protocol::Udp => f.write_str("udp"),
        }
    }
}

/// The process that holds a socket open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

/// One row of the socket table.
#[derive(Debug, Clone)]
pub struct PortEntry {
    pub protocol: Protocol,
    pub local: SocketAddr,
    pub remote: SocketAddr,
    /// Kernel socket state, e.g. "Listen", "Established", "TimeWait".
    pub state: String,
    /// True for a TCP socket in LISTEN state, or an unconnected UDP socket.
    pub listening: bool,
    pub inode: u64,
    /// Owner of the socket as recorded by the kernel (user name, or the
    /// numeric uid if it has no name).
    pub user: String,
    /// `None` when the owning process could not be found, which usually
    /// means it belongs to another user and we are not root.
    pub process: Option<ProcessInfo>,
}

/// Identity of a socket across scans. Used to tell which rows are new,
/// which disappeared, and to keep the dashboard cursor on the same row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryKey {
    pub protocol: Protocol,
    pub local: SocketAddr,
    pub remote: SocketAddr,
    pub inode: u64,
}

impl PortEntry {
    pub fn key(&self) -> EntryKey {
        EntryKey {
            protocol: self.protocol,
            local: self.local,
            remote: self.remote,
            inode: self.inode,
        }
    }
}

/// Scan all TCP and UDP sockets (IPv4 and IPv6).
pub fn scan() -> Result<Vec<PortEntry>, procfs::ProcError> {
    let owners = socket_owners();
    let mut entries = Vec::new();

    // IPv6 tables may be missing when IPv6 is disabled, so only the IPv4
    // tables are required.
    let tcp_rows = tcp()?.into_iter().chain(tcp6().unwrap_or_default());
    for row in tcp_rows {
        entries.push(PortEntry {
            protocol: Protocol::Tcp,
            local: row.local_address,
            remote: row.remote_address,
            state: format!("{:?}", row.state),
            listening: row.state == TcpState::Listen,
            inode: row.inode,
            user: user_name(row.uid),
            process: owners.get(&row.inode).cloned(),
        });
    }

    let udp_rows = udp()?.into_iter().chain(udp6().unwrap_or_default());
    for row in udp_rows {
        // UDP has no LISTEN state: an unconnected socket (no remote
        // peer) is the equivalent of a server waiting for packets.
        let unconnected = row.remote_address.port() == 0;
        entries.push(PortEntry {
            protocol: Protocol::Udp,
            local: row.local_address,
            remote: row.remote_address,
            state: if unconnected {
                "Bound".to_string()
            } else {
                "Connected".to_string()
            },
            listening: unconnected,
            inode: row.inode,
            user: user_name(row.uid),
            process: owners.get(&row.inode).cloned(),
        });
    }

    entries.sort_by_key(|e| (e.protocol, e.local.port(), e.local.ip()));
    Ok(entries)
}

/// Map socket inode -> owning process, by walking every readable
/// `/proc/<pid>/fd`. Processes we are not allowed to inspect, or that exit
/// while we scan, are skipped.
fn socket_owners() -> HashMap<u64, ProcessInfo> {
    let mut owners = HashMap::new();
    let Ok(processes) = all_processes() else {
        return owners;
    };

    for process in processes.flatten() {
        let Ok(fds) = process.fd() else { continue };
        let mut info: Option<ProcessInfo> = None;

        for fd in fds.flatten() {
            if let FDTarget::Socket(inode) = fd.target {
                let info = info.get_or_insert_with(|| ProcessInfo {
                    pid: process.pid as u32,
                    name: process
                        .stat()
                        .map(|s| s.comm)
                        .unwrap_or_else(|_| "?".to_string()),
                });
                // A socket can be shared by several processes (fork);
                // keep the first one found.
                owners.entry(inode).or_insert_with(|| info.clone());
            }
        }
    }
    owners
}

fn user_name(uid: u32) -> String {
    nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_else(|| uid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, UdpSocket};

    #[test]
    fn finds_our_own_tcp_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let entries = scan().unwrap();
        let entry = entries
            .iter()
            .find(|e| e.protocol == Protocol::Tcp && e.local.port() == port)
            .expect("our TCP listener should appear in the scan");

        assert!(entry.listening);
        assert_eq!(entry.state, "Listen");
        let owner = entry.process.as_ref().expect("we own this socket");
        assert_eq!(owner.pid, std::process::id());
    }

    #[test]
    fn finds_our_own_udp_socket() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();

        let entries = scan().unwrap();
        let entry = entries
            .iter()
            .find(|e| e.protocol == Protocol::Udp && e.local.port() == port)
            .expect("our UDP socket should appear in the scan");

        assert!(entry.listening);
        assert_eq!(
            entry.process.as_ref().map(|p| p.pid),
            Some(std::process::id())
        );
    }

    #[test]
    fn established_connections_are_not_listening() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _client = std::net::TcpStream::connect(addr).unwrap();
        let (_server_side, _) = listener.accept().unwrap();

        let entries = scan().unwrap();
        let established = entries
            .iter()
            .filter(|e| e.protocol == Protocol::Tcp && e.state == "Established")
            .filter(|e| e.local.port() == addr.port() || e.remote.port() == addr.port())
            .count();
        assert!(established >= 2, "client and server ends should both show up");
        assert!(
            entries
                .iter()
                .filter(|e| e.state == "Established")
                .all(|e| !e.listening)
        );
    }
}
