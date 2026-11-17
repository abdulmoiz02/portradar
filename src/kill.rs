//! Stopping the process that owns a port, with safety checks.

use std::fmt;
use std::fs;

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

#[derive(Debug, PartialEq, Eq)]
pub enum KillError {
    /// PID 1 or portradar itself.
    Protected(&'static str),
    /// The process has already exited.
    Gone,
    /// The PID now belongs to a different program than the one we showed.
    Changed { expected: String, found: String },
    /// Same name, but it started at a different time: the PID was reused.
    Restarted,
    /// Not allowed to signal this process (it belongs to another user).
    NotPermitted,
    Other(String),
}

impl fmt::Display for KillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KillError::Protected(why) => write!(f, "refusing to kill: {why}"),
            KillError::Gone => write!(f, "process already exited"),
            KillError::Changed { expected, found } => write!(
                f,
                "PID now belongs to '{found}' instead of '{expected}'; not killing"
            ),
            KillError::Restarted => {
                write!(f, "PID was reused by a newer process with the same name; not killing")
            }
            KillError::NotPermitted => write!(
                f,
                "permission denied (owned by another user; try running portradar with sudo)"
            ),
            KillError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// PIDs that must never be signalled.
pub fn protected_reason(pid: u32, own_pid: u32) -> Option<&'static str> {
    if pid == 1 {
        Some("PID 1 is the init system")
    } else if pid == own_pid {
        Some("that is portradar itself")
    } else {
        None
    }
}

/// Send SIGTERM (or SIGKILL when `force`) to `pid`, after checking that it
/// is still the process we showed: same name and, when known, same start
/// time (PIDs can be reused).
pub fn stop(
    pid: u32,
    expected_name: &str,
    started_at: Option<u64>,
    force: bool,
) -> Result<(), KillError> {
    stop_with(pid, expected_name, started_at, force, std::process::id())
}

fn stop_with(
    pid: u32,
    expected_name: &str,
    started_at: Option<u64>,
    force: bool,
    own_pid: u32,
) -> Result<(), KillError> {
    if let Some(why) = protected_reason(pid, own_pid) {
        return Err(KillError::Protected(why));
    }

    let comm = fs::read_to_string(format!("/proc/{pid}/comm")).map_err(|_| KillError::Gone)?;
    let comm = comm.trim_end();
    if comm != expected_name {
        return Err(KillError::Changed {
            expected: expected_name.to_string(),
            found: comm.to_string(),
        });
    }

    // shortcut: 5 s of slack, because the boot time can shift a little when the
    // clock is corrected; a reused PID within 5 s of the old process is not realistic.
    if let (Some(shown), Some(now)) = (started_at, crate::scanner::started_at(pid)) {
        if shown.abs_diff(now) > 5 {
            return Err(KillError::Restarted);
        }
    }

    let signal = if force { Signal::SIGKILL } else { Signal::SIGTERM };
    kill(Pid::from_raw(pid as i32), signal).map_err(|errno| match errno {
        Errno::EPERM => KillError::NotPermitted,
        Errno::ESRCH => KillError::Gone,
        other => KillError::Other(other.to_string()),
    })
}

/// Start `sleep 60` and wait until it has really become `sleep`: right after
/// a fork the child can still carry the parent's name in /proc/<pid>/comm,
/// which our PID-reuse check would (correctly) refuse to signal.
#[cfg(test)]
pub(crate) fn spawn_sleeper() -> std::process::Child {
    let child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    for _ in 0..200 {
        let comm = fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap_or_default();
        if comm.trim_end() == "sleep" {
            return child;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("sleep process never started");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    #[test]
    fn refuses_init_and_self() {
        assert!(protected_reason(1, 500).is_some());
        assert!(protected_reason(500, 500).is_some());
        assert!(protected_reason(501, 500).is_none());
        assert!(matches!(
            stop_with(1, "systemd", None, false, 500),
            Err(KillError::Protected(_))
        ));
    }

    #[test]
    fn terminates_a_real_process() {
        let mut child = spawn_sleeper();
        stop(child.id(), "sleep", None, false).unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(Signal::SIGTERM as i32));
    }

    #[test]
    fn force_kills_with_sigkill() {
        let mut child = spawn_sleeper();
        stop(child.id(), "sleep", None, true).unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(Signal::SIGKILL as i32));
    }

    #[test]
    fn refuses_when_pid_belongs_to_a_different_program() {
        let mut child = spawn_sleeper();
        let err = stop(child.id(), "node", None, false).unwrap_err();
        assert!(matches!(err, KillError::Changed { .. }));
        // Still alive: clean up ourselves.
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn refuses_a_same_named_process_that_started_at_another_time() {
        let mut child = spawn_sleeper();
        let real = crate::scanner::started_at(child.id()).unwrap();
        assert_eq!(stop(child.id(), "sleep", Some(real.saturating_sub(3600)), false), Err(KillError::Restarted));
        // The matching start time is accepted.
        stop(child.id(), "sleep", Some(real), false).unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn reports_a_process_that_is_already_gone() {
        let mut child = Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert_eq!(stop(pid, "true", None, false), Err(KillError::Gone));
    }
}
