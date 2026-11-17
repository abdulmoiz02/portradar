//! "Did you know?" lines: a fact about the selected port, or a tip about
//! portradar itself when there is no fact to give.
//!
//! Facts are kept deliberately conservative: default ports and what the
//! protocol is for. No dates, no history.

use crate::scanner::Protocol;

/// How long each general tip stays before the next one.
pub const ROTATE_SECS: u64 = 15;

pub fn fact_for(protocol: Protocol, port: u16) -> Option<&'static str> {
    let udp = protocol == Protocol::Udp;
    Some(match port {
        22 if !udp => "SSH is the standard way to log in to another computer. It is encrypted.",
        25 if !udp => "SMTP is how mail servers hand email to each other.",
        53 => "DNS is the internet's phone book: it turns names like example.com into numbers.",
        80 if !udp => "HTTP is the original web protocol. Plain HTTP is not encrypted.",
        123 if udp => "NTP keeps your clock in step with the rest of the world.",
        443 if !udp => "HTTPS is HTTP inside encryption: the padlock in your browser.",
        631 => "CUPS is the Linux printing system. This port is how programs talk to it.",
        1883 if !udp => "MQTT is a light messaging protocol, popular with smart-home gadgets.",
        3000 if !udp => "3000 is a favourite for Node and Rails dev servers. A habit, not a rule.",
        3306 if !udp => "3306 is MySQL's default port. Databases are usually kept local-only.",
        5173 if !udp => "5173 is the default port of the Vite dev server.",
        5353 if udp => "mDNS lets devices find each other by name on a network, with no server.",
        5432 if !udp => "5432 is PostgreSQL's default port.",
        6379 if !udp => "6379 is Redis's default port. Redis is often used as a fast cache.",
        8080 if !udp => "8080 is a common alternative to 80 for web servers and proxies.",
        9200 if !udp => "9200 is Elasticsearch's default port.",
        27017 if !udp => "27017 is MongoDB's default port.",
        _ => return None,
    })
}

pub const GENERAL: &[&str] = &[
    "Any program can listen on any port number. The usual numbers are only habits.",
    "Press / and type mysql, network or a port number to filter the table.",
    "Press e to show only the ports other devices on your network can reach.",
    "Press Enter on a row to open the Inspector: full technical detail.",
    "Press f to free a port: portradar stops its program, then checks it is free.",
    "Press : to open the command menu and find any action by name.",
    "Press t to switch between the simple and the technical table.",
    "A port bound to 127.0.0.1 can only be reached from this computer.",
    "Press h to hide system noise like DNS, printing and discovery.",
    "Press T to try another colour theme.",
];

/// The line to show: a fact about the selected port if there is one,
/// otherwise a general tip that rotates with `elapsed_secs`.
pub fn pick(selected: Option<(Protocol, u16)>, elapsed_secs: u64) -> &'static str {
    if let Some((protocol, port)) = selected {
        if let Some(fact) = fact_for(protocol, port) {
            return fact;
        }
    }
    GENERAL[(elapsed_secs / ROTATE_SECS) as usize % GENERAL.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_ports_have_facts_and_unknown_ones_do_not() {
        assert!(fact_for(Protocol::Tcp, 22).unwrap().contains("SSH"));
        assert!(fact_for(Protocol::Udp, 53).unwrap().contains("phone book"));
        assert!(fact_for(Protocol::Tcp, 53).is_some());
        assert!(fact_for(Protocol::Tcp, 54321).is_none());
    }

    #[test]
    fn protocol_specific_facts_are_not_claimed_for_the_wrong_protocol() {
        assert!(fact_for(Protocol::Udp, 22).is_none());
        assert!(fact_for(Protocol::Tcp, 5353).is_none());
        assert!(fact_for(Protocol::Udp, 5353).is_some());
        assert!(fact_for(Protocol::Tcp, 123).is_none());
    }

    #[test]
    fn a_port_fact_beats_the_general_tips() {
        assert_eq!(pick(Some((Protocol::Tcp, 6379)), 0), fact_for(Protocol::Tcp, 6379).unwrap());
        assert_eq!(pick(Some((Protocol::Tcp, 6379)), 9_999), fact_for(Protocol::Tcp, 6379).unwrap());
    }

    #[test]
    fn general_tips_rotate_with_time_and_wrap_around() {
        assert_eq!(pick(None, 0), GENERAL[0]);
        assert_eq!(pick(None, ROTATE_SECS - 1), GENERAL[0]);
        assert_eq!(pick(None, ROTATE_SECS), GENERAL[1]);
        assert_eq!(pick(None, ROTATE_SECS * GENERAL.len() as u64), GENERAL[0]);
        // A port with no fact falls back to the rotation.
        assert_eq!(pick(Some((Protocol::Tcp, 54321)), 0), GENERAL[0]);
    }

    #[test]
    fn every_line_is_short_plain_ascii_text() {
        let all = GENERAL.iter().copied().chain((1..=65535u16).flat_map(|p| {
            [Protocol::Tcp, Protocol::Udp].into_iter().filter_map(move |proto| fact_for(proto, p))
        }));
        for line in all {
            assert!(line.is_ascii(), "{line}");
            assert!(line.len() <= 90, "too long for one line: {line}");
        }
    }
}
