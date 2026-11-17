//! Extra information about the process that owns a port: its command line,
//! how long it has been running, and whether it is managed by Docker or
//! systemd (in which case killing it is usually the wrong fix).

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use procfs::process::Process;

/// Why killing this process directly may not do what you want.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// Process runs inside a container (short container id).
    Container(String),
    /// `docker-proxy` forwards a published container port.
    DockerProxy,
    /// Process belongs to a systemd system service, e.g. `nginx.service`.
    Systemd(String),
}

impl Hint {
    /// One-sentence advice, suitable for the details panel and the kill popup.
    pub fn advice(&self) -> String {
        match self {
            Hint::Container(id) => format!(
                "Runs in container {id}. Stop it with `docker stop {id}`; killing the process may just make Docker restart it."
            ),
            Hint::DockerProxy => "docker-proxy forwards a published container port. Stop the container instead (`docker ps` to find it).".to_string(),
            Hint::Systemd(unit) => format!(
                "Managed by systemd ({unit}); it may restart after a kill. Use `sudo systemctl stop {unit}`."
            ),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Details {
    pub cmdline: String,
    pub cwd: Option<String>,
    pub running_for_secs: Option<u64>,
    pub hint: Option<Hint>,
}

/// Gather details for a PID. Anything we are not allowed to read is left
/// empty rather than treated as an error.
pub fn inspect(pid: u32, process_name: &str) -> Details {
    let Ok(process) = Process::new(pid as i32) else {
        return Details::default();
    };

    let cmdline = process
        .cmdline()
        .map(|parts| parts.join(" "))
        .unwrap_or_default();
    let cwd = process.cwd().ok().map(|p| p.display().to_string());
    let running_for_secs = crate::scanner::started_at(pid).and_then(|started| {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(now.saturating_sub(started))
    });
    let hint = classify(
        process_name,
        &fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default(),
    );

    Details {
        cmdline,
        cwd,
        running_for_secs,
        hint,
    }
}

/// Decide from the process name and its `/proc/<pid>/cgroup` text whether
/// it is a container process or a systemd service.
pub fn classify(process_name: &str, cgroup: &str) -> Option<Hint> {
    if process_name == "docker-proxy" {
        return Some(Hint::DockerProxy);
    }

    for line in cgroup.lines() {
        // Format: "hierarchy-id:controllers:path"; the path is last.
        let path = line.rsplit(':').next().unwrap_or("");
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        for (i, seg) in segments.iter().enumerate() {
            // Docker with the systemd cgroup driver: docker-<id>.scope
            if let Some(id) = seg
                .strip_prefix("docker-")
                .and_then(|s| s.strip_suffix(".scope"))
            {
                return Some(Hint::Container(short_id(id)));
            }
            // Docker with the cgroupfs driver: /docker/<id>
            if *seg == "docker" {
                if let Some(id) = segments.get(i + 1) {
                    return Some(Hint::Container(short_id(id)));
                }
            }
        }

        // A system service: /system.slice/<name>.service
        if let Some(i) = segments.iter().position(|s| *s == "system.slice") {
            if let Some(unit) = segments.get(i + 1).filter(|s| s.ends_with(".service")) {
                return Some(Hint::Systemd(unit.to_string()));
            }
        }
    }
    None
}

fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

/// Very short form for table cells: "42s", "5m", "3h", "12d".
pub fn short_duration(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

pub fn human_duration(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    if secs >= DAY {
        format!("{}d {}h", secs / DAY, (secs % DAY) / HOUR)
    } else if secs >= HOUR {
        format!("{}h {}m", secs / HOUR, (secs % HOUR) / MIN)
    } else if secs >= MIN {
        format!("{}m {}s", secs / MIN, secs % MIN)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_systemd_system_service() {
        let cg = "0::/system.slice/nginx.service\n";
        assert_eq!(
            classify("nginx", cg),
            Some(Hint::Systemd("nginx.service".to_string()))
        );
    }

    #[test]
    fn detects_docker_container_systemd_driver() {
        let id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cg = format!("0::/system.slice/docker-{id}.scope\n");
        assert_eq!(
            classify("redis-server", &cg),
            Some(Hint::Container("0123456789ab".to_string()))
        );
    }

    #[test]
    fn detects_docker_container_cgroupfs_driver() {
        let cg = "12:memory:/docker/abcdef0123456789abcdef\n";
        assert_eq!(
            classify("app", cg),
            Some(Hint::Container("abcdef012345".to_string()))
        );
    }

    #[test]
    fn detects_docker_proxy_by_name() {
        assert_eq!(classify("docker-proxy", ""), Some(Hint::DockerProxy));
    }

    #[test]
    fn user_session_processes_get_no_hint() {
        // A user's own app must not be mistaken for a system service just
        // because the user manager is itself a .service.
        let cg = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-code.scope\n";
        assert_eq!(classify("code", cg), None);
        assert_eq!(classify("bash", ""), None);
    }

    #[test]
    fn formats_short_durations() {
        assert_eq!(short_duration(7), "7s");
        assert_eq!(short_duration(59), "59s");
        assert_eq!(short_duration(600), "10m");
        assert_eq!(short_duration(7_300), "2h");
        assert_eq!(short_duration(5 * 86_400 + 100), "5d");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(human_duration(5), "5s");
        assert_eq!(human_duration(125), "2m 5s");
        assert_eq!(human_duration(3 * 3600 + 7 * 60), "3h 7m");
        assert_eq!(human_duration(2 * 86400 + 5 * 3600), "2d 5h");
    }

    #[test]
    fn inspects_our_own_process() {
        let d = inspect(std::process::id(), "portradar");
        assert!(!d.cmdline.is_empty());
        assert!(d.running_for_secs.is_some());
    }
}
