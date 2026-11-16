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
}

impl ThemeName {
    pub fn parse(name: &str) -> Option<ThemeName> {
        match name.trim().to_lowercase().as_str() {
            "default" | "dark" => Some(ThemeName::Default),
            "light" => Some(ThemeName::Light),
            "mono" | "monochrome" => Some(ThemeName::Mono),
            _ => None,
        }
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
    };
}

#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub theme: Theme,
    pub glyphs: Glyphs,
    pub ascii: bool,
}

impl Look {
    pub fn new(theme: ThemeName, ascii: bool) -> Look {
        Look {
            theme: Theme::named(theme),
            glyphs: if ascii { Glyphs::ASCII } else { Glyphs::UNICODE },
            ascii,
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
    fn ascii_glyphs_really_are_ascii() {
        let g = Glyphs::ASCII;
        for s in [
            g.logo, g.live, g.paused, g.warn, g.row_bar, g.local, g.whole, g.one, g.up, g.down,
            g.keys_move, g.keys_tabs, g.dot, g.cursor, g.normal, g.dev, g.mask, g.quote_open,
            g.quote_close,
        ] {
            assert!(s.is_ascii(), "{s:?} is not ASCII");
        }
    }
}
