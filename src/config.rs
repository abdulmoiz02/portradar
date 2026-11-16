//! Optional settings from `~/.config/portwatch/config.toml`, plus a tiny
//! "have you seen the tour" marker.
//!
//! portwatch only ever *reads* the config file. It never creates or changes
//! it, so running under `sudo` cannot leave root-owned files in your home.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::theme::{Look, ThemeName};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Seconds between refreshes (1 to 30).
    pub refresh_seconds: u64,
    /// Use plain ASCII symbols and borders (for terminals or fonts without Unicode).
    pub ascii: bool,
    /// Let the mouse click tabs and rows and scroll the table.
    pub mouse: bool,
    /// "default", "light", "mono", "nord", "dracula", "cyberpunk", "matrix" or "colorblind".
    pub theme: String,
    /// Motion: the radar sweep, new-port ripples and the confetti. Turn off
    /// for a perfectly still screen.
    pub animations: bool,
    /// The "Did you know?" line under the table.
    pub tips: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            refresh_seconds: 1,
            ascii: false,
            mouse: true,
            theme: "default".to_string(),
            animations: true,
            tips: true,
        }
    }
}

pub const SAMPLE: &str = "\
# portwatch settings. Every line is optional; delete any you don't need.
refresh_seconds = 1     # how often the table refreshes (1 to 30)
theme = \"default\"       # default, light, mono, nord, dracula, cyberpunk, matrix, colorblind
ascii = false           # true = plain ASCII symbols and borders
mouse = true            # click tabs and rows, scroll with the wheel
animations = true       # false = no radar sweep, ripples or confetti
tips = true             # false = hide the \"Did you know?\" line
";

impl Config {
    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh_seconds.clamp(1, 30))
    }

    pub fn look(&self) -> Look {
        let theme = ThemeName::parse(&self.theme).unwrap_or(ThemeName::Default);
        Look::new(theme, self.ascii)
    }
}

pub struct Loaded {
    pub config: Config,
    /// Why the file was ignored (shown once in the footer), if it was.
    pub warning: Option<String>,
    pub path: Option<PathBuf>,
}

/// Parse and validate settings text.
pub fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if !(1..=30).contains(&config.refresh_seconds) {
        return Err(format!(
            "refresh_seconds is {}, but it must be between 1 and 30",
            config.refresh_seconds
        ));
    }
    if ThemeName::parse(&config.theme).is_none() {
        return Err(format!(
            "unknown theme \"{}\" (use default, light, mono, nord, dracula, cyberpunk, matrix or colorblind)",
            config.theme
        ));
    }
    Ok(config)
}

pub fn config_path() -> Option<PathBuf> {
    dir_from_env("XDG_CONFIG_HOME", ".config").map(|d| d.join("portwatch").join("config.toml"))
}

pub fn state_dir() -> Option<PathBuf> {
    dir_from_env("XDG_STATE_HOME", ".local/state").map(|d| d.join("portwatch"))
}

fn dir_from_env(xdg_var: &str, home_default: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(xdg_var).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(home).join(home_default))
}

/// Load settings. A missing file is normal (defaults). A broken file never
/// stops portwatch: defaults are used and a warning is returned.
pub fn load() -> Loaded {
    let path = config_path();
    let Some(file) = path.clone().filter(|p| p.exists()) else {
        return Loaded { config: Config::default(), warning: None, path };
    };
    let result = fs::read_to_string(&file)
        .map_err(|e| e.to_string())
        .and_then(|text| parse(&text));
    match result {
        Ok(config) => Loaded { config, warning: None, path },
        Err(why) => Loaded {
            config: Config::default(),
            warning: Some(format!("Ignoring {}: {why}", file.display())),
            path,
        },
    }
}

const TOUR_MARKER: &str = "tour-seen";

pub fn tour_seen_in(dir: &Path) -> bool {
    dir.join(TOUR_MARKER).exists()
}

/// Remember that the tour was shown. Errors are ignored: the worst case is
/// seeing the tour again.
pub fn mark_tour_seen_in(dir: &Path) {
    if fs::create_dir_all(dir).is_ok() {
        let _ = fs::write(dir.join(TOUR_MARKER), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn the_sample_file_is_valid_and_matches_the_defaults() {
        assert_eq!(parse(SAMPLE).unwrap(), Config::default());
    }

    #[test]
    fn reads_every_setting() {
        let c = parse(
            "refresh_seconds = 5\nascii = true\nmouse = false\ntheme = \"light\"\nanimations = false\ntips = false",
        )
        .unwrap();
        assert!(!c.animations && !c.tips);
        assert_eq!(c.refresh(), Duration::from_secs(5));
        assert!(c.ascii);
        assert!(!c.mouse);
        assert_eq!(c.theme, "light");
        assert!(c.look().ascii);
    }

    #[test]
    fn rejects_bad_values_with_a_clear_reason() {
        assert!(parse("refresh_seconds = 0").unwrap_err().contains("between 1 and 30"));
        assert!(parse("refresh_seconds = 99").unwrap_err().contains("between 1 and 30"));
        assert!(parse("theme = \"neon\"").unwrap_err().contains("unknown theme"));
        // A typo in a key is reported instead of silently ignored.
        assert!(parse("refresh_second = 2").is_err());
        assert!(parse("this is not toml").is_err());
    }

    #[test]
    fn tour_marker_round_trips_in_a_directory() {
        let dir = std::env::temp_dir().join(format!("portwatch-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(!tour_seen_in(&dir));
        mark_tour_seen_in(&dir);
        assert!(tour_seen_in(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
