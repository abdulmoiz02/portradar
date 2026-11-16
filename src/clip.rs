//! Copying text to the clipboard from a terminal program.
//!
//! There is no single reliable way, so we try, in order:
//! 1. `wl-copy` (Wayland), `xclip` or `xsel` (X11), if installed;
//! 2. the terminal's own clipboard escape code (OSC 52), which many
//!    terminals ignore. We can never tell whether that one worked.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// How long to wait for a clipboard tool before assuming it has detached to
/// keep serving the clipboard (as wl-copy and xclip do).
const TOOL_WAIT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyResult {
    /// A clipboard program accepted the text.
    Tool(String),
    /// No program worked; the caller should try the terminal escape code.
    NoTool,
}

/// Programs worth trying here, in order, as (name, arguments).
fn candidates(wayland: bool, x11: bool) -> Vec<(&'static str, &'static [&'static str])> {
    let mut list: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if wayland {
        list.push(("wl-copy", &[]));
    }
    if x11 {
        list.push(("xclip", &["-selection", "clipboard"]));
        list.push(("xsel", &["--clipboard", "--input"]));
    }
    list
}

/// Try the clipboard programs on a background thread and report back. The
/// interface never blocks, and nothing is claimed that did not happen.
pub fn copy_in_background(text: String, tx: Sender<CopyResult>) {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    thread::spawn(move || {
        for (program, args) in candidates(wayland, x11) {
            if try_tool(program, args, &text) {
                let _ = tx.send(CopyResult::Tool(program.to_string()));
                return;
            }
        }
        let _ = tx.send(CopyResult::NoTool);
    });
}

/// Feed `text` to one clipboard program. Only stdin is connected, and it is
/// closed before waiting; otherwise the program would wait for more input
/// forever.
fn try_tool(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false; // not installed
    };

    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    if !written {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }

    // These tools normally exit quickly after handing the text to a
    // background process. One that is still running after the wait is
    // serving the clipboard, which is what we want.
    let deadline = Instant::now() + TOOL_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
                return true;
            }
            Err(_) => return false,
        }
    }
}

/// The OSC 52 escape sequence that asks a terminal to copy `text`.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_4648_examples() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected, "{input:?}");
        }
    }

    #[test]
    fn osc52_wraps_the_encoded_text() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn tools_are_chosen_by_what_the_session_offers() {
        assert!(candidates(false, false).is_empty());
        let wayland: Vec<_> = candidates(true, false).iter().map(|c| c.0).collect();
        assert_eq!(wayland, vec!["wl-copy"]);
        let both: Vec<_> = candidates(true, true).iter().map(|c| c.0).collect();
        assert_eq!(both, vec!["wl-copy", "xclip", "xsel"]);
    }

    #[test]
    fn a_working_tool_is_used_and_fed_the_text() {
        // `cat > file` accepts stdin and exits: a stand-in for xclip.
        let dir = std::env::temp_dir().join(format!("portwatch-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("copied.txt");
        let ok = try_tool("sh", &["-c", &format!("cat > {}", out.display())], "hello clipboard");
        assert!(ok);
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello clipboard");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_failing_tool_is_not_success() {
        assert!(!try_tool("portwatch-no-such-program", &[], "x"));
        assert!(!try_tool("sh", &["-c", "cat >/dev/null; exit 3"], "x"));
    }

    #[test]
    fn a_tool_that_keeps_running_counts_as_the_clipboard_owner() {
        // Reads stdin, then keeps running like wl-copy serving a selection.
        let started = Instant::now();
        let ok = try_tool("sh", &["-c", "cat >/dev/null; sleep 5"], "x");
        assert!(ok);
        assert!(started.elapsed() < Duration::from_secs(4), "must not wait for the tool");
    }
}
