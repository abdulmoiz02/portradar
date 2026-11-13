//! Plain-English helpers: what a port number is usually used for, and who
//! can reach a socket bound to a given address.

use std::net::IpAddr;

use crate::scanner::Protocol;

/// Who can connect to a socket, judged from the address it is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    /// Bound to loopback (127.x.x.x or ::1): only programs on this computer.
    ThisPcOnly,
    /// Bound to one specific network address.
    OneNetwork,
    /// Bound to 0.0.0.0 or :: (every network interface).
    WholeNetwork,
}

impl Access {
    pub fn of(ip: IpAddr) -> Access {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            v4 => v4,
        };
        if ip.is_loopback() {
            Access::ThisPcOnly
        } else if ip.is_unspecified() {
            Access::WholeNetwork
        } else {
            Access::OneNetwork
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Access::ThisPcOnly => "this PC only",
            Access::OneNetwork => "one network",
            Access::WholeNetwork => "whole network",
        }
    }

    /// True when something other than this computer may be able to connect.
    pub fn is_exposed(self) -> bool {
        self != Access::ThisPcOnly
    }

    /// A full sentence for the details panel.
    pub fn sentence(self, ip: IpAddr) -> String {
        match self {
            Access::ThisPcOnly => "Only this computer can reach it.".to_string(),
            Access::OneNetwork => format!("It is reachable from the network through {ip}."),
            Access::WholeNetwork => {
                "Anything on your network can try to connect to it.".to_string()
            }
        }
    }
}

/// What this port number is normally used for. This is a guess based on the
/// number alone: any program can listen on any port.
pub fn service_name(protocol: Protocol, port: u16) -> Option<&'static str> {
    let name = match port {
        20 | 21 => "FTP",
        22 => "SSH",
        23 => "Telnet",
        25 | 465 | 587 => "Mail (SMTP)",
        53 => "DNS",
        67 | 68 | 546 | 547 => "DHCP",
        80 => "Web (HTTP)",
        110 | 995 => "Mail (POP3)",
        123 => "Time sync (NTP)",
        143 | 993 => "Mail (IMAP)",
        443 => "Web (HTTPS)",
        445 => "File sharing",
        631 => "Printing (CUPS)",
        1883 => "MQTT",
        1900 => "Device discovery",
        2375 | 2376 => "Docker API",
        3000 | 3001 | 5000 | 8000 => "Dev server",
        3306 | 33060 => "MySQL",
        3389 => "Remote desktop",
        4200 => "Angular dev",
        5173 => "Vite dev",
        5353 => "mDNS discovery",
        5355 => "LLMNR lookup",
        5432 => "PostgreSQL",
        5601 => "Kibana",
        5672 => "RabbitMQ",
        5900 => "VNC",
        6379 => "Redis",
        6443 => "Kubernetes API",
        8080 | 8888 => "Web (alt)",
        8443 => "Web (HTTPS alt)",
        9090 => "Prometheus",
        9200 => "Elasticsearch",
        11211 => "Memcached",
        27017 => "MongoDB",
        _ => return None,
    };
    // 5353, 1900, 5355 and the DHCP ports are UDP services; TCP on those
    // numbers is not what the name says.
    let udp_only = matches!(port, 67 | 68 | 546 | 547 | 1900 | 5353 | 5355);
    if udp_only && protocol == Protocol::Tcp {
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn loopback_is_this_pc_only() {
        assert_eq!(Access::of(ip("127.0.0.1")), Access::ThisPcOnly);
        assert_eq!(Access::of(ip("127.0.0.53")), Access::ThisPcOnly);
        assert_eq!(Access::of(ip("::1")), Access::ThisPcOnly);
        // IPv4 loopback written as an IPv6-mapped address.
        assert_eq!(Access::of(ip("::ffff:127.0.0.1")), Access::ThisPcOnly);
    }

    #[test]
    fn wildcard_addresses_are_the_whole_network() {
        assert_eq!(Access::of(ip("0.0.0.0")), Access::WholeNetwork);
        assert_eq!(Access::of(ip("::")), Access::WholeNetwork);
    }

    #[test]
    fn a_specific_address_is_one_network() {
        assert_eq!(Access::of(ip("192.168.1.20")), Access::OneNetwork);
        assert_eq!(Access::of(ip("fe80::1")), Access::OneNetwork);
        assert_eq!(Access::of(ip("224.0.0.251")), Access::OneNetwork);
    }

    #[test]
    fn only_loopback_is_not_exposed() {
        assert!(!Access::ThisPcOnly.is_exposed());
        assert!(Access::OneNetwork.is_exposed());
        assert!(Access::WholeNetwork.is_exposed());
        assert!(Access::ThisPcOnly < Access::OneNetwork);
        assert!(Access::OneNetwork < Access::WholeNetwork);
    }

    #[test]
    fn names_well_known_ports() {
        assert_eq!(service_name(Protocol::Tcp, 5432), Some("PostgreSQL"));
        assert_eq!(service_name(Protocol::Tcp, 6379), Some("Redis"));
        assert_eq!(service_name(Protocol::Udp, 53), Some("DNS"));
        assert_eq!(service_name(Protocol::Tcp, 54321), None);
    }

    #[test]
    fn udp_only_services_are_not_claimed_for_tcp() {
        assert_eq!(service_name(Protocol::Udp, 5353), Some("mDNS discovery"));
        assert_eq!(service_name(Protocol::Tcp, 5353), None);
    }
}
