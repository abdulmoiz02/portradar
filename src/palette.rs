//! The command menu (`:`): every action by name, searchable, so nobody has
//! to remember the keys.

use crate::app::Tab;
use crate::model::SortKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Stop,
    ForceStop,
    Free,
    Open,
    Inspect,
    ToggleTechnical,
    SortBy(SortKey),
    Reverse,
    ToggleExposed,
    ToggleMine,
    ToggleQuiet,
    Pause,
    Goto(Tab),
    ClearSearch,
    Help,
    Tour,
    Quit,
}

#[derive(Debug)]
pub struct Entry {
    pub name: &'static str,
    /// The shortcut key for this action, shown at the right.
    pub key: &'static str,
    pub command: Command,
}

pub const COMMANDS: &[Entry] = &[
    Entry { name: "Stop the selected program", key: "k", command: Command::Stop },
    Entry { name: "Force stop the selected program", key: "K", command: Command::ForceStop },
    Entry { name: "Free the selected port", key: "f", command: Command::Free },
    Entry { name: "Open the selected port in the browser", key: "o", command: Command::Open },
    Entry { name: "Inspect the selected program (full technical detail)", key: "Enter", command: Command::Inspect },
    Entry { name: "Switch between simple and technical table", key: "t", command: Command::ToggleTechnical },
    Entry { name: "Show only ports open to the network (toggle)", key: "e", command: Command::ToggleExposed },
    Entry { name: "Show only my programs (toggle)", key: "m", command: Command::ToggleMine },
    Entry { name: "Hide system noise (toggle)", key: "h", command: Command::ToggleQuiet },
    Entry { name: "Clear the search", key: "Esc", command: Command::ClearSearch },
    Entry { name: "Sort by port", key: "s", command: Command::SortBy(SortKey::Port) },
    Entry { name: "Sort by program name", key: "s", command: Command::SortBy(SortKey::Process) },
    Entry { name: "Sort by PID", key: "s", command: Command::SortBy(SortKey::Pid) },
    Entry { name: "Sort by user", key: "s", command: Command::SortBy(SortKey::User) },
    Entry { name: "Sort by uptime (newest first)", key: "s", command: Command::SortBy(SortKey::Uptime) },
    Entry { name: "Sort by access (most local first)", key: "s", command: Command::SortBy(SortKey::Access) },
    Entry { name: "Reverse the sort order", key: "r", command: Command::Reverse },
    Entry { name: "Pause or resume refreshing", key: "p", command: Command::Pause },
    Entry { name: "Go to Listening ports", key: "1", command: Command::Goto(Tab::Listening) },
    Entry { name: "Go to Connections", key: "2", command: Command::Goto(Tab::Connections) },
    Entry { name: "Go to Events", key: "3", command: Command::Goto(Tab::Events) },
    Entry { name: "Show the help", key: "?", command: Command::Help },
    Entry { name: "Show the welcome tour again", key: "", command: Command::Tour },
    Entry { name: "Quit portwatch", key: "q", command: Command::Quit },
];

/// Commands whose name contains every word typed (any case). An empty
/// search lists everything.
pub fn matching(input: &str) -> Vec<&'static Entry> {
    let words: Vec<String> = input.split_whitespace().map(|w| w.to_lowercase()).collect();
    COMMANDS
        .iter()
        .filter(|entry| {
            let name = entry.name.to_lowercase();
            words.iter().all(|w| name.contains(w))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_search_lists_everything() {
        assert_eq!(matching("").len(), COMMANDS.len());
        assert_eq!(matching("   ").len(), COMMANDS.len());
    }

    #[test]
    fn words_can_be_in_any_order_and_any_case() {
        let found = matching("PROGRAM stop");
        assert!(found.iter().any(|e| e.command == Command::Stop));
        assert!(found.iter().any(|e| e.command == Command::ForceStop));
        assert!(matching("uptime").iter().all(|e| e.command == Command::SortBy(SortKey::Uptime)));
    }

    #[test]
    fn nonsense_finds_nothing() {
        assert!(matching("zzzzz").is_empty());
    }

    #[test]
    fn every_sort_key_and_tab_has_a_command() {
        for key in [
            SortKey::Port,
            SortKey::Process,
            SortKey::Pid,
            SortKey::User,
            SortKey::Uptime,
            SortKey::Access,
        ] {
            assert!(COMMANDS.iter().any(|e| e.command == Command::SortBy(key)), "{key:?}");
        }
        for tab in Tab::ALL {
            assert!(COMMANDS.iter().any(|e| e.command == Command::Goto(tab)), "{tab:?}");
        }
    }
}
