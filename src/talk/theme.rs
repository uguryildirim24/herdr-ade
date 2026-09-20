//! One catppuccin palette, overlaid per token by herdr's custom settings.
use crate::paths::Env;
use ratatui::style::Color;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Theme {
    pub(crate) accent: Color,
    pub(crate) text: Color,
    pub(crate) subtext0: Color,
    pub(crate) overlay0: Color,
    pub(crate) surface1: Color,
    pub(crate) panel_bg: Color,
    pub(crate) green: Color,
    pub(crate) yellow: Color,
    pub(crate) red: Color,
    pub(crate) peach: Color,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Rgb(203, 166, 247),
            text: Color::Rgb(205, 214, 244),
            subtext0: Color::Rgb(166, 173, 200),
            overlay0: Color::Rgb(108, 112, 134),
            surface1: Color::Rgb(69, 71, 90),
            panel_bg: Color::Rgb(24, 24, 37),
            green: Color::Rgb(166, 227, 161),
            yellow: Color::Rgb(249, 226, 175),
            red: Color::Rgb(243, 139, 168),
            peach: Color::Rgb(250, 179, 135),
        }
    }
}
impl Theme {
    pub(crate) fn load(env: &Env) -> Self {
        let base = env
            .var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    env.var("APPDATA")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| env.home.join(".config"))
                } else {
                    env.home.join(".config")
                }
            });
        Self::parse(&std::fs::read_to_string(base.join("herdr/config.toml")).unwrap_or_default())
    }
    fn parse(text: &str) -> Self {
        let mut t = Self::default();
        let Ok(value) = toml::from_str::<toml::Value>(text) else {
            return t;
        };
        let Some(custom) = value.get("theme").and_then(|v| v.get("custom")) else {
            return t;
        };
        for (name, token) in [
            ("accent", &mut t.accent),
            ("text", &mut t.text),
            ("subtext0", &mut t.subtext0),
            ("overlay0", &mut t.overlay0),
            ("surface1", &mut t.surface1),
            ("panel_bg", &mut t.panel_bg),
            ("green", &mut t.green),
            ("yellow", &mut t.yellow),
            ("red", &mut t.red),
            ("peach", &mut t.peach),
        ] {
            if let Some(color) = custom.get(name).and_then(|v| v.as_str()).and_then(color) {
                *token = color;
            }
        }
        t
    }
}

/// Accept the same color spellings as herdr. Unlike herdr's general parser,
/// an unknown value returns `None` so this screen can keep that token's
/// carried fallback as the project-screen contract requires.
fn color(value: &str) -> Option<Color> {
    let value = value.trim().to_ascii_lowercase();
    if matches!(value.as_str(), "reset" | "default" | "none" | "transparent") {
        return Some(Color::Reset);
    }
    if let Some(hex) = value.strip_prefix('#')
        && hex.is_ascii()
    {
        match hex.len() {
            6 => {
                let n = u32::from_str_radix(hex, 16).ok()?;
                return Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
            }
            3 => {
                let mut digits = hex.chars().map(|c| c.to_digit(16).map(|n| n as u8));
                let (r, g, b) = (digits.next()??, digits.next()??, digits.next()??);
                return Some(Color::Rgb(r * 17, g * 17, b * 17));
            }
            _ => {}
        }
    }
    if let Some(inner) = value
        .strip_prefix("rgb(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let mut parts = inner.split(',').map(str::trim);
        let (r, g, b) = (
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        if parts.next().is_none() {
            return Some(Color::Rgb(r, g, b));
        }
    }
    Some(match value.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => return None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn herdr_color_spellings_and_per_token_fallback() {
        let t = Theme::parse(
            "[theme.custom]\naccent = '#123456'\ntext = 'rgb(1, 2, 3)'\ngreen = '#0f8'\nyellow = 'blue'\npanel_bg = 'default'\nred = 'bad'\n",
        );
        assert_eq!(t.accent, Color::Rgb(18, 52, 86));
        assert_eq!(t.text, Color::Rgb(1, 2, 3));
        assert_eq!(t.green, Color::Rgb(0, 255, 136));
        assert_eq!(t.yellow, Color::Blue);
        assert_eq!(t.panel_bg, Color::Reset);
        assert_eq!(t.red, Theme::default().red);
        assert_eq!(Theme::parse("not toml").accent, Theme::default().accent);
    }
}
