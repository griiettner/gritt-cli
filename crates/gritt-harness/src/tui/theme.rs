//! Semantic theme tokens for the full-screen mode.
//!
//! Widgets ask for a role (`accent`, `muted`, `error`) and never for a
//! colour, so a palette change is one edit here. Three palettes exist:
//! dark, light, and a no-colour palette that carries modifiers only, used
//! when `NO_COLOR` is set. `NO_COLOR` is honoured by dropping every
//! foreground and background colour, not by choosing a dim colour.

use ratatui::style::{Color, Modifier, Style};

/// Which palette the interface draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    /// `NO_COLOR` is set: shape, text, and modifiers carry every meaning.
    NoColor,
}

impl ThemeMode {
    /// The mode for this process: `NO_COLOR` wins over everything, then
    /// `GRITT_THEME=light|dark`, then dark.
    pub fn from_env<I, K, V>(vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut mode = ThemeMode::Dark;
        for (key, value) in vars {
            match key.as_ref() {
                // Any value, including the empty string, disables colour.
                "NO_COLOR" => return ThemeMode::NoColor,
                "GRITT_THEME" if value.as_ref().eq_ignore_ascii_case("light") => {
                    mode = ThemeMode::Light;
                }
                "GRITT_THEME" if value.as_ref().eq_ignore_ascii_case("dark") => {
                    mode = ThemeMode::Dark;
                }
                _ => {}
            }
        }
        mode
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
            ThemeMode::NoColor => "no-color",
        }
    }
}

/// The eight semantic tokens every widget draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub background: Color,
    /// Panels and rows lifted off the background: the composer, overlays.
    pub surface: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub selection: Color,
    pub success: Color,
    pub error: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub mode: ThemeMode,
    palette: Palette,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new(ThemeMode::Dark)
    }
}

const DARK: Palette = Palette {
    background: Color::Rgb(0x14, 0x16, 0x1a),
    surface: Color::Rgb(0x1e, 0x21, 0x27),
    text: Color::Rgb(0xdd, 0xe1, 0xe6),
    muted: Color::Rgb(0x87, 0x8f, 0x9c),
    accent: Color::Rgb(0x7a, 0xa2, 0xf7),
    selection: Color::Rgb(0x2d, 0x3f, 0x64),
    success: Color::Rgb(0x7d, 0xcf, 0x8a),
    error: Color::Rgb(0xf0, 0x71, 0x78),
};

const LIGHT: Palette = Palette {
    background: Color::Rgb(0xfa, 0xfa, 0xf8),
    surface: Color::Rgb(0xee, 0xef, 0xf2),
    text: Color::Rgb(0x22, 0x25, 0x2b),
    muted: Color::Rgb(0x5f, 0x66, 0x72),
    accent: Color::Rgb(0x2b, 0x54, 0xa8),
    selection: Color::Rgb(0xcd, 0xda, 0xf5),
    success: Color::Rgb(0x1d, 0x6f, 0x35),
    error: Color::Rgb(0xa3, 0x1d, 0x2c),
};

impl Theme {
    pub fn new(mode: ThemeMode) -> Self {
        let palette = match mode {
            ThemeMode::Dark => DARK,
            ThemeMode::Light => LIGHT,
            // Never read: `styled` drops colour in this mode. Kept so the
            // struct has one shape and `palette()` is always answerable.
            ThemeMode::NoColor => DARK,
        };
        Self { mode, palette }
    }

    pub fn from_env<I, K, V>(vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Theme::new(ThemeMode::from_env(vars))
    }

    pub fn uses_color(&self) -> bool {
        self.mode != ThemeMode::NoColor
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    fn styled(&self, fg: Color, modifier: Modifier) -> Style {
        let style = Style::default().add_modifier(modifier);
        if self.uses_color() {
            style.fg(fg)
        } else {
            style
        }
    }

    /// The whole-screen background. Empty in no-colour mode.
    pub fn screen(&self) -> Style {
        if self.uses_color() {
            Style::default()
                .bg(self.palette.background)
                .fg(self.palette.text)
        } else {
            Style::default()
        }
    }

    /// A panel raised off the background: overlays and the composer.
    pub fn raised(&self) -> Style {
        if self.uses_color() {
            Style::default()
                .bg(self.palette.surface)
                .fg(self.palette.text)
        } else {
            Style::default()
        }
    }

    pub fn text(&self) -> Style {
        self.styled(self.palette.text, Modifier::empty())
    }

    pub fn muted(&self) -> Style {
        self.styled(self.palette.muted, Modifier::empty())
    }

    pub fn accent(&self) -> Style {
        self.styled(self.palette.accent, Modifier::empty())
    }

    pub fn heading(&self) -> Style {
        self.styled(self.palette.accent, Modifier::BOLD)
    }

    pub fn success(&self) -> Style {
        self.styled(self.palette.success, Modifier::empty())
    }

    pub fn error(&self) -> Style {
        self.styled(self.palette.error, Modifier::BOLD)
    }

    /// The highlighted row of a picker or suggestion list. Reversed in
    /// every mode so the selection is visible without colour.
    pub fn selection(&self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.uses_color() {
            style.bg(self.palette.selection).fg(self.palette.text)
        } else {
            style.add_modifier(Modifier::REVERSED)
        }
    }

    /// A disabled or unavailable row.
    pub fn dim(&self) -> Style {
        self.styled(self.palette.muted, Modifier::DIM)
    }

    pub fn reasoning(&self) -> Style {
        self.styled(self.palette.muted, Modifier::ITALIC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_wins_over_a_requested_palette() {
        let theme = Theme::from_env([("GRITT_THEME", "light"), ("NO_COLOR", "")]);
        assert_eq!(theme.mode, ThemeMode::NoColor);
        assert!(!theme.uses_color());
        // Not one token may carry a colour in this mode.
        for style in [
            theme.text(),
            theme.muted(),
            theme.accent(),
            theme.error(),
            theme.success(),
            theme.selection(),
            theme.raised(),
            theme.screen(),
            theme.heading(),
            theme.dim(),
            theme.reasoning(),
        ] {
            assert_eq!(style.fg, None, "{style:?}");
            assert_eq!(style.bg, None, "{style:?}");
        }
        // The selection is still distinguishable.
        assert!(theme.selection().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn light_and_dark_are_selected_by_name_and_differ() {
        assert_eq!(
            ThemeMode::from_env([("GRITT_THEME", "Light")]),
            ThemeMode::Light
        );
        assert_eq!(
            ThemeMode::from_env([("GRITT_THEME", "dark")]),
            ThemeMode::Dark
        );
        assert_eq!(ThemeMode::from_env([("PATH", "/bin")]), ThemeMode::Dark);
        let dark = Theme::new(ThemeMode::Dark);
        let light = Theme::new(ThemeMode::Light);
        assert_ne!(dark.palette().background, light.palette().background);
        assert_ne!(dark.text().fg, light.text().fg);
        assert_eq!(dark.mode.name(), "dark");
        assert_eq!(light.mode.name(), "light");
    }
}
