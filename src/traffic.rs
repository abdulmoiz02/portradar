//! Live internet usage: overall speed, per-connection TCP speed, and the
//! machine's own IP addresses.
//!
//! Only TCP has per-socket byte counters in Linux (`ss -ti`). UDP and QUIC
//! have none without root-level packet capture, so those are not measured
//! per connection. The overall figure comes from the network cards and does
//! include them.

use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::process::Command;
use std::time::{Duration, Instant};

/// Bytes per second.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Speed {
    pub down: f64,
    pub up: f64,
}

/// One reading of all the counters.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub at: Option<Instant>,
    /// Bytes received and sent per physical network card.
    pub cards: HashMap<String, (u64, u64)>,
    /// Bytes received and sent (and acknowledged) per TCP socket, by inode.
    /// `None` when `ss` is not installed.
    pub tcp: Option<HashMap<u64, (u64, u64)>>,
}

pub fn sample() -> Sample {
    Sample {
        at: Some(Instant::now()),
        cards: fs::read_to_string("/proc/net/dev")
            .map(|text| parse_net_dev(&text, is_physical))
            .unwrap_or_default(),
        tcp: Command::new("ss")
            .arg("-tinHOe")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| parse_ss(&String::from_utf8_lossy(&out.stdout))),
    }
}

/// A real network card has a `device` link in sysfs. Loopback, Docker
/// bridges, VPN tunnels and the like do not, and counting them would count
/// the same bytes twice.
fn is_physical(name: &str) -> bool {
    std::path::Path::new("/sys/class/net").join(name).join("device").exists()
}

/// `/proc/net/dev`: one line per card, `name: rx_bytes ... tx_bytes ...`.
pub fn parse_net_dev(text: &str, keep: impl Fn(&str) -> bool) -> HashMap<String, (u64, u64)> {
    let mut cards = HashMap::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        let name = name.trim();
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // 8 receive columns, then the transmit ones; bytes come first in each.
        let (Some(rx), Some(tx)) = (fields.first(), fields.get(8)) else { continue };
        if let (Ok(rx), Ok(tx)) = (rx.parse(), tx.parse())
            && keep(name)
        {
            cards.insert(name.to_string(), (rx, tx));
        }
    }
    cards
}

/// `ss -tinHOe`: one line per socket. We only need the socket's inode (to
/// match our own rows) and its byte counters. `bytes_acked` is what the peer
/// confirmed, so unlike `bytes_sent` it leaves out retransmissions. `ss`
/// leaves a counter out while it is zero.
pub fn parse_ss(text: &str) -> HashMap<u64, (u64, u64)> {
    let mut sockets = HashMap::new();
    for line in text.lines() {
        let (mut inode, mut down, mut up) = (0u64, 0u64, 0u64);
        for word in line.split_whitespace() {
            let number = |prefix: &str| word.strip_prefix(prefix).and_then(|n| n.parse().ok());
            if let Some(n) = number("ino:") {
                inode = n;
            } else if let Some(n) = number("bytes_received:") {
                down = n;
            } else if let Some(n) = number("bytes_acked:") {
                up = n;
            }
        }
        if inode != 0 {
            sockets.insert(inode, (down, up));
        }
    }
    sockets
}

/// Turns raw counters into speeds.
#[derive(Debug, Default)]
pub struct Meter {
    prev: Option<Sample>,
    /// Overall speed, `None` until two readings exist.
    pub overall: Option<Speed>,
    /// Bytes moved since the first reading.
    pub total_down: u64,
    pub total_up: u64,
    /// TCP speed per socket inode. A socket seen for the first time has none.
    pub speeds: HashMap<u64, Speed>,
    /// False when `ss` is missing, so the per-connection columns stay hidden.
    pub per_connection: bool,
}

impl Meter {
    pub fn update(&mut self, now: Sample) {
        self.per_connection = now.tcp.is_some();
        if let Some(prev) = &self.prev {
            let secs = match (now.at, prev.at) {
                (Some(at), Some(before)) => at.saturating_duration_since(before).as_secs_f64(),
                _ => 0.0,
            };
            if secs >= 0.2 {
                let (mut down, mut up) = (0u64, 0u64);
                for (name, &(rx, tx)) in &now.cards {
                    // A counter that went down was reset: skip that card once.
                    if let Some(&(prx, ptx)) = prev.cards.get(name)
                        && rx >= prx
                        && tx >= ptx
                    {
                        down += rx - prx;
                        up += tx - ptx;
                    }
                }
                self.total_down += down;
                self.total_up += up;
                self.overall = Some(Speed { down: down as f64 / secs, up: up as f64 / secs });

                self.speeds.clear();
                if let (Some(old), Some(new)) = (&prev.tcp, &now.tcp) {
                    for (inode, &(rx, tx)) in new {
                        if let Some(&(prx, ptx)) = old.get(inode)
                            && rx >= prx
                            && tx >= ptx
                        {
                            let speed = Speed { down: (rx - prx) as f64 / secs, up: (tx - ptx) as f64 / secs };
                            self.speeds.insert(*inode, speed);
                        }
                    }
                }
            } else {
                // Too soon to say anything: keep the old baseline and speeds.
                return;
            }
        }
        self.prev = Some(now);
    }

    /// Forget the baseline (while paused, the gap would be averaged into one
    /// misleading speed).
    pub fn pause(&mut self) {
        self.prev = None;
        self.overall = None;
        self.speeds.clear();
    }
}

/// `1.2 MB/s`, `340 KB/s`, `0 B/s`. Decimal units, like network tools use.
pub fn human_speed(bytes_per_second: f64) -> String {
    format!("{}/s", human_bytes(bytes_per_second as u64))
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

// ---- this machine's addresses ------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalIps {
    /// The address on the card that reaches the internet, and that card.
    pub private: Option<(Ipv4Addr, String)>,
    /// A globally routable IPv6 address, if the machine has one.
    pub ipv6: Option<Ipv6Addr>,
}

/// Name of the card that holds the default route (the way out).
fn default_interface(route_table: &str) -> Option<String> {
    route_table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let metric: u32 = f.get(6)?.parse().ok()?;
            (f.get(1) == Some(&"00000000") && f.get(7) == Some(&"00000000"))
                .then(|| (metric, f[0].to_string()))
        })
        .min()
        .map(|(_, name)| name)
}

fn is_global_v6(ip: &Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || first & 0xffc0 == 0xfe80 // link-local
        || first & 0xfe00 == 0xfc00) // unique local (private)
}

/// Pick the addresses to show from `(card, address)` pairs.
pub fn choose_ips(
    addresses: &[(String, IpAddr)],
    default_card: Option<&str>,
    physical: impl Fn(&str) -> bool,
) -> LocalIps {
    let usable = |card: &str| physical(card);
    let v4 = |preferred: bool| {
        addresses.iter().find_map(|(card, ip)| match ip {
            IpAddr::V4(v4)
                if usable(card) && (!preferred || Some(card.as_str()) == default_card) =>
            {
                Some((*v4, card.clone()))
            }
            _ => None,
        })
    };
    LocalIps {
        private: v4(true).or_else(|| v4(false)),
        ipv6: addresses.iter().find_map(|(card, ip)| match ip {
            IpAddr::V6(v6) if usable(card) && is_global_v6(v6) => Some(*v6),
            _ => None,
        }),
    }
}

pub fn local_ips() -> LocalIps {
    let route = fs::read_to_string("/proc/net/route").unwrap_or_default();
    let addresses: Vec<(String, IpAddr)> = nix::ifaddrs::getifaddrs()
        .map(|list| {
            list.filter_map(|a| {
                let addr = a.address?;
                let ip = match (addr.as_sockaddr_in(), addr.as_sockaddr_in6()) {
                    (Some(v4), _) => IpAddr::V4(v4.ip()),
                    (_, Some(v6)) => IpAddr::V6(v6.ip()),
                    _ => return None,
                };
                Some((a.interface_name, ip))
            })
            .collect()
        })
        .unwrap_or_default();
    choose_ips(&addresses, default_interface(&route).as_deref(), is_physical)
}

// ---- public IPv4 ------------------------------------------------------------------

/// The outside servers we ask (both at once, the first answer wins, because
/// some networks block one of them). OpenDNS answers a DNS question for
/// `myip.opendns.com` with the address the question came from.
const RESOLVERS: [SocketAddr; 2] = [
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(208, 67, 222, 222)), 53),
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(208, 67, 220, 220)), 53),
];
const QUESTION_NAME: &str = "myip.opendns.com";

/// A DNS question for the A record of `name`.
pub fn dns_query(id: u16, name: &str) -> Vec<u8> {
    let mut packet = Vec::with_capacity(32);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // recursion wanted, 1 question
    for label in name.split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 1, 0, 1]); // end of name, type A, class IN
    packet
}

/// Skip a DNS name (plain labels, or a 2-byte pointer) and return where it ends.
fn skip_name(packet: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *packet.get(at)? as usize;
        match len {
            0 => return Some(at + 1),
            l if l & 0xc0 == 0xc0 => return Some(at + 2),
            _ => at += 1 + len,
        }
    }
}

/// The first IPv4 address in the answer section of a reply to `dns_query`.
pub fn parse_dns_answer(packet: &[u8], id: u16) -> Result<Ipv4Addr, String> {
    let bad = || "unexpected reply".to_string();
    if packet.len() < 12 || packet[..2] != id.to_be_bytes() || packet[2] & 0x80 == 0 {
        return Err(bad());
    }
    if packet[3] & 0x0f != 0 {
        return Err("the server refused the question".to_string());
    }
    let questions = u16::from_be_bytes([packet[4], packet[5]]);
    let answers = u16::from_be_bytes([packet[6], packet[7]]);
    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(packet, at).ok_or_else(bad)? + 4; // type and class
    }
    for _ in 0..answers {
        at = skip_name(packet, at).ok_or_else(bad)?;
        let header = packet.get(at..at + 10).ok_or_else(bad)?;
        let (kind, length) = (u16::from_be_bytes([header[0], header[1]]), u16::from_be_bytes([header[8], header[9]]));
        let data = packet.get(at + 10..at + 10 + length as usize).ok_or_else(bad)?;
        if kind == 1 && data.len() == 4 {
            return Ok(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
        }
        at += 10 + length as usize;
    }
    Err("no address in the reply".to_string())
}

/// Ask the internet what our public IPv4 address is. Blocks for up to a few
/// seconds, so call it from a background thread.
pub fn public_ipv4() -> Result<Ipv4Addr, String> {
    let io = |e: std::io::Error| format!("no answer ({e})");
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(io)?;
    let id = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(7)
        & 0xffff) as u16;
    let query = dns_query(id, QUESTION_NAME);
    for server in RESOLVERS {
        socket.send_to(&query, server).map_err(io)?;
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut last_problem = "no answer (timed out)".to_string();
    let mut buffer = [0u8; 512];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        socket.set_read_timeout(Some(left)).map_err(io)?;
        let Ok((n, from)) = socket.recv_from(&mut buffer) else { break };
        if !RESOLVERS.contains(&from) {
            continue; // not from the servers we asked
        }
        match parse_dns_answer(&buffer[..n], id) {
            Ok(ip) => return Ok(ip),
            Err(problem) => last_problem = problem,
        }
    }
    Err(last_problem)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 7620253   51226    0    0    0     0          0         0  7620253   51226    0    0    0     0       0          0
wlp3s0:123456789  9999    0    0    0     0          0         0   555000    4444    0    0    0     0       0          0
docker0:       5       1    0    0    0     0          0         0        9       1    0    0    0     0       0          0
";

    #[test]
    fn net_dev_keeps_only_wanted_cards() {
        let cards = parse_net_dev(NET_DEV, |name| name == "wlp3s0");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards["wlp3s0"], (123456789, 555000));
        assert_eq!(parse_net_dev(NET_DEV, |_| true).len(), 3);
    }

    #[test]
    fn ss_matches_by_inode_and_uses_acked_bytes() {
        let text = "\
ESTAB 0 0 192.168.18.145%wlp3s0:54574 13.227.180.4:443 timer:(keepalive,10sec,0) uid:1000 ino:545435 sk:2 cgroup:/user.slice/a.scope <-> ts sack cubic bytes_sent:28989 bytes_retrans:1000 bytes_acked:27990 bytes_received:900000 segs_out:5
ESTAB 0 0 [2001:db8::1]:40000 [2001:db8::2]:443 uid:1000 ino:700 sk:3 <-> cubic bytes_received:42
ESTAB 0 0 10.0.0.1:1 10.0.0.2:2 uid:0 ino:0 sk:4 <-> cubic bytes_received:7
";
        let sockets = parse_ss(text);
        assert_eq!(sockets.len(), 2, "ino:0 rows are skipped");
        assert_eq!(sockets[&545435], (900000, 27990)); // not bytes_sent
        assert_eq!(sockets[&700], (42, 0)); // a missing counter means zero
    }

    fn sample_at(base: Instant, secs: u64, card: (u64, u64), tcp: &[(u64, (u64, u64))]) -> Sample {
        Sample {
            at: Some(base + Duration::from_secs(secs)),
            cards: HashMap::from([("wlp3s0".to_string(), card)]),
            tcp: Some(tcp.iter().copied().collect()),
        }
    }

    #[test]
    fn meter_computes_speeds_and_totals() {
        let base = Instant::now();
        let mut meter = Meter::default();
        meter.update(sample_at(base, 0, (1000, 500), &[(1, (0, 0))]));
        assert_eq!(meter.overall, None, "needs two readings");
        meter.update(sample_at(base, 2, (5000, 1500), &[(1, (2000, 100)), (2, (9, 9))]));
        assert_eq!(meter.overall, Some(Speed { down: 2000.0, up: 500.0 }));
        assert_eq!((meter.total_down, meter.total_up), (4000, 1000));
        assert_eq!(meter.speeds[&1], Speed { down: 1000.0, up: 50.0 });
        assert!(!meter.speeds.contains_key(&2), "a new socket has no speed yet");
        assert!(meter.per_connection);
    }

    #[test]
    fn meter_survives_counter_resets_and_pauses() {
        let base = Instant::now();
        let mut meter = Meter::default();
        meter.update(sample_at(base, 0, (9000, 9000), &[(1, (500, 500))]));
        meter.update(sample_at(base, 1, (100, 100), &[(1, (10, 10))]));
        assert_eq!(meter.overall, Some(Speed::default()), "reset card counts as nothing");
        assert!(meter.speeds.is_empty(), "reset socket has no speed");
        meter.pause();
        meter.update(sample_at(base, 100, (200, 200), &[]));
        assert_eq!(meter.overall, None, "first reading after a pause only sets the baseline");
    }

    #[test]
    fn without_ss_there_are_no_per_connection_speeds() {
        let base = Instant::now();
        let mut meter = Meter::default();
        let mut s = sample_at(base, 0, (0, 0), &[]);
        s.tcp = None;
        meter.update(s);
        assert!(!meter.per_connection);
    }

    #[test]
    fn readable_sizes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1500), "1.5 KB");
        assert_eq!(human_bytes(340_000), "340 KB");
        assert_eq!(human_bytes(1_234_567), "1.2 MB");
        assert_eq!(human_bytes(999_999), "1.0 MB");
        assert_eq!(human_speed(2_500_000.0), "2.5 MB/s");
    }

    #[test]
    fn default_route_wins_by_metric() {
        let table = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
wlp3s0\t00000000\t0112A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
enp0s31f6\t00000000\t0112A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n";
        assert_eq!(default_interface(table).as_deref(), Some("enp0s31f6"));
        assert_eq!(default_interface("Iface\tDestination\n"), None);
    }

    #[test]
    fn global_ipv6_excludes_local_kinds() {
        assert!(is_global_v6(&"2001:db8::1".parse().unwrap()));
        assert!(!is_global_v6(&"fe80::1".parse().unwrap()));
        assert!(!is_global_v6(&"fd00::1".parse().unwrap()));
        assert!(!is_global_v6(&"::1".parse().unwrap()));
    }

    #[test]
    fn own_addresses_prefer_the_default_card() {
        let addresses: Vec<(String, IpAddr)> = [
            ("lo", "127.0.0.1"),
            ("docker0", "172.17.0.1"),
            ("enp0s31f6", "10.1.1.9"),
            ("wlp3s0", "192.168.1.5"),
            ("wlp3s0", "fe80::1"),
            ("wlp3s0", "2001:db8::5"),
        ]
        .iter()
        .map(|(card, ip)| (card.to_string(), ip.parse().unwrap()))
        .collect();
        let physical = |card: &str| card.starts_with("wlp") || card.starts_with("enp");
        let ips = choose_ips(&addresses, Some("wlp3s0"), physical);
        assert_eq!(ips.private, Some(("192.168.1.5".parse().unwrap(), "wlp3s0".to_string())));
        assert_eq!(ips.ipv6, Some("2001:db8::5".parse().unwrap()));
        // No default route: first physical card wins, never docker or lo.
        let ips = choose_ips(&addresses, None, physical);
        assert_eq!(ips.private.unwrap().1, "enp0s31f6");
    }

    #[test]
    fn dns_query_has_the_right_shape() {
        let q = dns_query(0xabcd, "myip.opendns.com");
        assert_eq!(&q[..4], &[0xab, 0xcd, 0x01, 0x00]);
        assert_eq!(&q[12..], b"\x04myip\x07opendns\x03com\x00\x00\x01\x00\x01");
    }

    #[test]
    fn dns_answer_is_parsed_and_checked() {
        let mut reply = dns_query(0x1234, "myip.opendns.com");
        reply[2] = 0x81; // response, recursion
        reply[3] = 0x80;
        reply[7] = 1; // one answer
        reply.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 203, 0, 113, 7]);
        assert_eq!(parse_dns_answer(&reply, 0x1234), Ok(Ipv4Addr::new(203, 0, 113, 7)));
        assert!(parse_dns_answer(&reply, 0x9999).is_err(), "wrong id");
        assert!(parse_dns_answer(&reply[..20], 0x1234).is_err(), "cut short");
        reply[3] = 0x83; // NXDOMAIN
        assert!(parse_dns_answer(&reply, 0x1234).is_err());
    }
}
