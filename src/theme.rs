//! Colours, symbols and borders. Everything the UI draws with comes from a
//! `Look`, so the theme and the ASCII-only mode apply everywhere.

use ratatui::style::Color;
use ratatui::symbols::border;
use ratatui::widgets::{Block, BorderType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    /// For dark terminals; uses the terminal's own palette for colours.
    Default,
    /// For light terminals.
    Light,
    /// Greys only. Meaning is carried by the symbols, not by colour.
    Mono,
    Nord,
    Dracula,
    Cyberpunk,
    /// Green on dark green, like a hacker film.
    Matrix,
    /// Blue and orange instead of green and red.
    ColorBlind,
}

impl ThemeName {
    /// Every theme, in the order the `T` key cycles through them.
    pub const ALL: [ThemeName; 8] = [
        ThemeName::Default,
        ThemeName::Light,
        ThemeName::Mono,
        ThemeName::Nord,
        ThemeName::Dracula,
        ThemeName::Cyberpunk,
        ThemeName::Matrix,
        ThemeName::ColorBlind,
    ];

    pub fn parse(name: &str) -> Option<ThemeName> {
        match name.trim().to_lowercase().as_str() {
            "default" | "dark" => Some(ThemeName::Default),
            "light" => Some(ThemeName::Light),
            "mono" | "monochrome" => Some(ThemeName::Mono),
            "nord" => Some(ThemeName::Nord),
            "dracula" => Some(ThemeName::Dracula),
            "cyberpunk" => Some(ThemeName::Cyberpunk),
            "matrix" | "hacker" => Some(ThemeName::Matrix),
            "colorblind" | "colourblind" | "color-blind" | "colour-blind" => Some(ThemeName::ColorBlind),
            _ => None,
        }
    }

    /// The name to put in the settings file.
    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Default => "default",
            ThemeName::Light => "light",
            ThemeName::Mono => "mono",
            ThemeName::Nord => "nord",
            ThemeName::Dracula => "dracula",
            ThemeName::Cyberpunk => "cyberpunk",
            ThemeName::Matrix => "matrix",
            ThemeName::ColorBlind => "colorblind",
        }
    }

    pub fn next(self) -> ThemeName {
        let i = ThemeName::ALL.iter().position(|t| *t == self).unwrap_or(0);
        ThemeName::ALL[(i + 1) % ThemeName::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    /// Text on an accent-coloured background.
    pub on_accent: Color,
    /// Secondary text (headers, hints, labels).
    pub dim: Color,
    /// Slightly stronger secondary text.
    pub subtle: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    pub bar_bg: Color,
    pub bar_fg: Color,
    pub chip_bg: Color,
    pub chip_fg: Color,
    pub select_bg: Color,
    pub select_fg: Color,
    /// Background washes for the radar's fading sweep, faint to strong.
    pub wake: [Color; 3],
    /// Radar line colours: the two inner rings and the outer ring.
    pub ring: [Color; 2],
    /// Radar bearing ticks: the short ones and the long ones.
    pub tick: [Color; 2],
    /// The radar's sweep beam.
    pub beam: Color,
    /// The same glow as a smooth gradient (faintest, strongest) for terminals
    /// that support 24-bit colour.
    pub wake_rgb: [(u8, u8, u8); 2],
}

impl Theme {
    pub fn named(name: ThemeName) -> Theme {
        match name {
            ThemeName::Default => Theme {
                accent: Color::Cyan,
                on_accent: Color::Black,
                dim: Color::DarkGray,
                subtle: Color::Gray,
                good: Color::Green,
                warn: Color::Yellow,
                bad: Color::Red,
                bar_bg: Color::Indexed(236),
                bar_fg: Color::White,
                chip_bg: Color::Indexed(238),
                chip_fg: Color::White,
                select_bg: Color::Indexed(238),
                select_fg: Color::White,
                wake: [Color::Indexed(22), Color::Indexed(22), Color::Indexed(22)],
                ring: [Color::Indexed(29), Color::Indexed(35)],
                tick: [Color::Indexed(22), Color::Indexed(35)],
                beam: Color::Indexed(121),
                wake_rgb: [(22, 48, 34), (40, 92, 64)],
            },
            ThemeName::Light => Theme {
                accent: Color::Blue,
                on_accent: Color::White,
                dim: Color::Indexed(244),
                subtle: Color::Indexed(240),
                good: Color::Green,
                warn: Color::Indexed(130),
                bad: Color::Red,
                bar_bg: Color::Indexed(252),
                bar_fg: Color::Black,
                chip_bg: Color::Indexed(252),
                chip_fg: Color::Black,
                select_bg: Color::Indexed(153),
                select_fg: Color::Black,
                wake: [Color::Indexed(194), Color::Indexed(194), Color::Indexed(194)],
                ring: [Color::Indexed(244), Color::Indexed(240)],
                tick: [Color::Indexed(244), Color::Indexed(240)],
                beam: Color::Blue,
                wake_rgb: [(228, 246, 232), (186, 228, 198)],
            },
            ThemeName::Nord => Theme {
                accent: Color::Indexed(110),
                on_accent: Color::Black,
                dim: Color::Indexed(60),
                subtle: Color::Indexed(146),
                good: Color::Indexed(108),
                warn: Color::Indexed(179),
                bad: Color::Indexed(131),
                bar_bg: Color::Indexed(236),
                bar_fg: Color::Indexed(253),
                chip_bg: Color::Indexed(238),
                chip_fg: Color::Indexed(253),
                select_bg: Color::Indexed(60),
                select_fg: Color::Indexed(255),
                wake: [Color::Indexed(23), Color::Indexed(23), Color::Indexed(23)],
                ring: [Color::Indexed(60), Color::Indexed(146)],
                tick: [Color::Indexed(60), Color::Indexed(146)],
                beam: Color::Indexed(110),
                wake_rgb: [(26, 50, 58), (40, 88, 100)],
            },
            ThemeName::Dracula => Theme {
                accent: Color::Indexed(141),
                on_accent: Color::Black,
                dim: Color::Indexed(60),
                subtle: Color::Indexed(103),
                good: Color::Indexed(84),
                warn: Color::Indexed(228),
                bad: Color::Indexed(203),
                bar_bg: Color::Indexed(236),
                bar_fg: Color::Indexed(255),
                chip_bg: Color::Indexed(238),
                chip_fg: Color::Indexed(255),
                select_bg: Color::Indexed(60),
                select_fg: Color::Indexed(255),
                wake: [Color::Indexed(53), Color::Indexed(53), Color::Indexed(53)],
                ring: [Color::Indexed(60), Color::Indexed(103)],
                tick: [Color::Indexed(60), Color::Indexed(103)],
                beam: Color::Indexed(141),
                wake_rgb: [(44, 30, 64), (84, 52, 124)],
            },
            ThemeName::Cyberpunk => Theme {
                accent: Color::Indexed(201),
                on_accent: Color::Black,
                dim: Color::Indexed(90),
                subtle: Color::Indexed(140),
                good: Color::Indexed(51),
                warn: Color::Indexed(226),
                bad: Color::Indexed(197),
                bar_bg: Color::Indexed(53),
                bar_fg: Color::Indexed(51),
                chip_bg: Color::Indexed(54),
                chip_fg: Color::Indexed(219),
                select_bg: Color::Indexed(54),
                select_fg: Color::Indexed(231),
                wake: [Color::Indexed(53), Color::Indexed(53), Color::Indexed(53)],
                ring: [Color::Indexed(90), Color::Indexed(140)],
                tick: [Color::Indexed(90), Color::Indexed(140)],
                beam: Color::Indexed(201),
                wake_rgb: [(58, 22, 70), (104, 30, 104)],
            },
            ThemeName::Matrix => Theme {
                accent: Color::Indexed(46),
                on_accent: Color::Black,
                dim: Color::Indexed(22),
                subtle: Color::Indexed(34),
                good: Color::Indexed(40),
                warn: Color::Indexed(190),
                bad: Color::Indexed(160),
                bar_bg: Color::Indexed(22),
                bar_fg: Color::Indexed(46),
                chip_bg: Color::Indexed(22),
                chip_fg: Color::Indexed(46),
                select_bg: Color::Indexed(22),
                select_fg: Color::Indexed(82),
                wake: [Color::Indexed(22), Color::Indexed(22), Color::Indexed(22)],
                ring: [Color::Indexed(28), Color::Indexed(34)],
                tick: [Color::Indexed(22), Color::Indexed(34)],
                beam: Color::Indexed(120),
                wake_rgb: [(8, 46, 18), (18, 92, 34)],
            },
            ThemeName::ColorBlind => Theme {
                accent: Color::Indexed(75),
                on_accent: Color::Black,
                dim: Color::DarkGray,
                subtle: Color::Gray,
                good: Color::Indexed(39),
                warn: Color::Indexed(214),
                bad: Color::Indexed(201),
                bar_bg: Color::Indexed(236),
                bar_fg: Color::White,
                chip_bg: Color::Indexed(238),
                chip_fg: Color::White,
                select_bg: Color::Indexed(238),
                select_fg: Color::White,
                wake: [Color::Indexed(17), Color::Indexed(17), Color::Indexed(17)],
                ring: [Color::DarkGray, Color::Gray],
                tick: [Color::DarkGray, Color::Gray],
                beam: Color::Indexed(75),
                wake_rgb: [(20, 36, 66), (30, 62, 112)],
            },
            ThemeName::Mono => Theme {
                accent: Color::White,
                on_accent: Color::Black,
                dim: Color::DarkGray,
                subtle: Color::Gray,
                good: Color::White,
                warn: Color::White,
                bad: Color::White,
                bar_bg: Color::Indexed(236),
                bar_fg: Color::White,
                chip_bg: Color::Indexed(238),
                chip_fg: Color::White,
                select_bg: Color::Indexed(238),
                select_fg: Color::White,
                wake: [Color::Indexed(236), Color::Indexed(236), Color::Indexed(236)],
                ring: [Color::DarkGray, Color::Gray],
                tick: [Color::DarkGray, Color::Gray],
                beam: Color::White,
                wake_rgb: [(44, 44, 44), (78, 78, 78)],
            },
        }
    }
}

/// Every symbol the UI uses, so ASCII mode can swap them all at once.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub logo: &'static str,
    pub live: &'static str,
    pub paused: &'static str,
    pub warn: &'static str,
    pub row_bar: &'static str,
    pub local: &'static str,
    pub whole: &'static str,
    pub one: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    pub keys_move: &'static str,
    pub keys_tabs: &'static str,
    pub dot: &'static str,
    pub cursor: &'static str,
    pub normal: &'static str,
    pub dev: &'static str,
    pub mask: &'static str,
    pub quote_open: &'static str,
    pub quote_close: &'static str,
    pub radar_ring: &'static str,
    pub radar_sweep: &'static str,
    pub radar_center: &'static str,
    pub radar_ripple: &'static str,
    /// Eight steps from low to high, for sparklines.
    pub spark: [&'static str; 8],
    pub confetti: [&'static str; 6],
}

impl Glyphs {
    pub const UNICODE: Glyphs = Glyphs {
        logo: "◉",
        live: "●",
        paused: "⏸",
        warn: "⚠",
        row_bar: "▌ ",
        local: "●",
        whole: "▲",
        one: "◆",
        up: "▲",
        down: "▼",
        keys_move: "↑↓",
        keys_tabs: "←→",
        dot: "·",
        cursor: "█",
        normal: "✓",
        dev: "★",
        mask: "•",
        quote_open: "“",
        quote_close: "”",
        radar_ring: "·",
        radar_sweep: "░",
        radar_center: "◎",
        radar_ripple: "○",
        spark: ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"],
        confetti: ["✦", "✧", "●", "◆", "▲", "·"],
    };

    pub const ASCII: Glyphs = Glyphs {
        logo: "*",
        live: "*",
        paused: "||",
        warn: "!",
        row_bar: "> ",
        local: "o",
        whole: "^",
        one: "+",
        up: "^",
        down: "v",
        keys_move: "Up/Dn",
        keys_tabs: "L/R",
        dot: "-",
        cursor: "_",
        normal: "+",
        dev: "*",
        mask: "*",
        quote_open: "\"",
        quote_close: "\"",
        radar_ring: ".",
        radar_sweep: ":",
        radar_center: "@",
        radar_ripple: "O",
        spark: [".", ":", "-", "=", "+", "*", "#", "%"],
        confetti: ["*", "+", "o", "x", "#", "."],
    };
}

#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub name: ThemeName,
    pub theme: Theme,
    pub glyphs: Glyphs,
    pub ascii: bool,
    /// The terminal can show 24-bit colours (smooth gradients).
    pub truecolor: bool,
}

impl Look {
    pub fn new(theme: ThemeName, ascii: bool) -> Look {
        Look {
            name: theme,
            theme: Theme::named(theme),
            glyphs: if ascii { Glyphs::ASCII } else { Glyphs::UNICODE },
            ascii,
            truecolor: false,
        }
    }

    /// The same look with another theme (keeps ASCII mode and colour depth).
    pub fn with_theme(&self, theme: ThemeName) -> Look {
        Look { truecolor: self.truecolor, ..Look::new(theme, self.ascii) }
    }

    /// The background for a cell the sweep's glow reaches: `level` runs from
    /// 1 (faint) to `WAKE_STEPS` (strongest); 0 means no glow.
    pub fn wake_color(&self, level: u8) -> Option<Color> {
        use crate::radar::WAKE_STEPS;
        if level == 0 {
            return None;
        }
        let level = level.min(WAKE_STEPS);
        if self.truecolor {
            let k = f32::from(level - 1) / f32::from(WAKE_STEPS - 1);
            let [(r0, g0, b0), (r1, g1, b1)] = self.theme.wake_rgb;
            let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * k).round() as u8;
            Some(Color::Rgb(lerp(r0, r1), lerp(g0, g1), lerp(b0, b1)))
        } else {
            Some(self.theme.wake[usize::from((level - 1) * 3 / WAKE_STEPS).min(2)])
        }
    }

    /// A bordered box in the current style (rounded, or plain ASCII).
    pub fn block(&self) -> Block<'static> {
        let block = Block::bordered();
        if self.ascii {
            block.border_set(border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            })
        } else {
            block.border_type(BorderType::Rounded)
        }
    }
}

impl Default for Look {
    fn default() -> Self {
        Look::new(ThemeName::Default, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_names() {
        assert_eq!(ThemeName::parse("default"), Some(ThemeName::Default));
        assert_eq!(ThemeName::parse(" Light "), Some(ThemeName::Light));
        assert_eq!(ThemeName::parse("MONO"), Some(ThemeName::Mono));
        assert_eq!(ThemeName::parse("neon"), None);
    }

    #[test]
    fn every_theme_has_a_name_that_parses_back_and_cycling_visits_all() {
        for theme in ThemeName::ALL {
            assert_eq!(ThemeName::parse(theme.label()), Some(theme), "{theme:?}");
        }
        let mut seen = vec![ThemeName::Default];
        let mut t = ThemeName::Default.next();
        while t != ThemeName::Default {
            seen.push(t);
            t = t.next();
        }
        assert_eq!(seen.len(), ThemeName::ALL.len());
        assert_eq!(ThemeName::parse("hacker"), Some(ThemeName::Matrix));
        assert_eq!(ThemeName::parse("Colour-Blind"), Some(ThemeName::ColorBlind));
    }

    #[test]
    fn the_wake_has_a_colour_for_every_strength_in_both_colour_depths() {
        use crate::radar::WAKE_STEPS;
        for theme in ThemeName::ALL {
            for truecolor in [false, true] {
                let look = Look { truecolor, ..Look::new(theme, false) };
                assert_eq!(look.wake_color(0), None);
                let colours: Vec<Color> = (1..=WAKE_STEPS).map(|l| look.wake_color(l).unwrap()).collect();
                assert_eq!(colours.len(), usize::from(WAKE_STEPS));
                if truecolor {
                    assert_ne!(colours[0], colours[colours.len() - 1], "{theme:?} has a gradient");
                } else {
                    assert!(colours.iter().all(|c| matches!(c, Color::Indexed(_))), "{theme:?} stays in 256 colours");
                }
            }
            assert_eq!(Look::new(theme, false).with_theme(ThemeName::Nord).name, ThemeName::Nord);
        }
        let look = Look { truecolor: true, ..Look::new(ThemeName::Default, true) };
        assert!(look.with_theme(ThemeName::Dracula).truecolor && look.with_theme(ThemeName::Dracula).ascii);
    }

    #[test]
    fn the_default_scope_is_phosphor_green_and_every_theme_has_a_visible_beam() {
        let t = Theme::named(ThemeName::Default);
        assert_eq!(t.beam, Color::Indexed(121), "light green beam");
        assert!(matches!(t.ring[0], Color::Indexed(28..=35)) && matches!(t.ring[1], Color::Indexed(28..=40)));
        for theme in ThemeName::ALL {
            let t = Theme::named(theme);
            assert_ne!(t.beam, t.ring[0], "{theme:?}: the beam stands out from the rings");
            assert_ne!(t.ring[0], t.wake[0], "{theme:?}: rings are visible over the glow");
        }
    }

    #[test]
    fn themes_use_256_colour_values_not_rgb() {
        for theme in ThemeName::ALL {
            let t = Theme::named(theme);
            for c in [t.accent, t.dim, t.subtle, t.good, t.warn, t.bad, t.bar_bg, t.bar_fg, t.chip_bg, t.chip_fg, t.select_bg, t.select_fg] {
                assert!(!matches!(c, Color::Rgb(..)), "{theme:?} uses RGB: {c:?}");
            }
        }
    }

    #[test]
    fn ascii_glyphs_really_are_ascii() {
        let g = Glyphs::ASCII;
        for s in [
            g.logo, g.live, g.paused, g.warn, g.row_bar, g.local, g.whole, g.one, g.up, g.down,
            g.keys_move, g.keys_tabs, g.dot, g.cursor, g.normal, g.dev, g.mask, g.quote_open,
            g.quote_close, g.radar_ring, g.radar_sweep, g.radar_center, g.radar_ripple,
        ]
        .into_iter()
        .chain(g.spark)
        .chain(g.confetti)
        {
            assert!(s.is_ascii(), "{s:?} is not ASCII");
        }
    }
}
