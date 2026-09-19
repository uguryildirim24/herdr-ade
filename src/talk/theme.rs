//! One catppuccin palette, overlaid per token by herdr's custom settings.
use crate::paths::Env;
use ratatui::style::Color;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Color,
    pub text: Color,
    pub subtext0: Color,
    pub overlay0: Color,
    pub surface1: Color,
    pub panel_bg: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub peach: Color,
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
    pub fn load(env: &Env) -> Self {
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
            if let Some(color) = custom.get(name).and_then(|v| v.as_str()).and_then(rgb) {
                *token = color;
            }
        }
        t
    }
}
fn rgb(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_accent_and_per_token_fallback() {
        let t = Theme::parse("[theme.custom]\naccent = '#123456'\ntext = 'bad'\n");
        assert_eq!(t.accent, Color::Rgb(18, 52, 86));
        assert_eq!(t.text, Theme::default().text);
        assert_eq!(Theme::parse("not toml").accent, Theme::default().accent);
    }
}
