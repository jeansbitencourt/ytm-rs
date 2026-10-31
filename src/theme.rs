use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RawThemeColors {
    pub name: Option<String>,
    pub mode: Option<String>,
    pub accent: Option<String>,
    pub background: Option<String>,
    pub dark_background: Option<String>,
    pub foreground: Option<String>,
    pub muted: Option<String>,
    pub selection: Option<String>,
    pub red: Option<String>,
    pub green: Option<String>,
    pub yellow: Option<String>,
    pub orange: Option<String>,
    pub cyan: Option<String>,
    pub blue: Option<String>,
    pub magenta: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub background: Color,
    pub dark_background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub selection: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub cyan: Color,
    pub magenta: Color,
    pub theme_name: String,
}

impl Default for Theme {
    fn default() -> Self {
        // Tokyo Night fallback
        Self {
            accent: Color::Rgb(122, 162, 247),       // #7aa2f7
            background: Color::Rgb(26, 27, 38),      // #1a1b26
            dark_background: Color::Rgb(20, 21, 30), // #14151e
            foreground: Color::Rgb(192, 202, 245),   // #c0caf5
            muted: Color::Rgb(86, 95, 137),          // #565f89
            selection: Color::Rgb(40, 52, 87),       // #283457
            red: Color::Rgb(247, 118, 142),          // #f7768e
            green: Color::Rgb(158, 206, 106),        // #9ece6a
            yellow: Color::Rgb(224, 175, 104),       // #e0af68
            cyan: Color::Rgb(125, 207, 255),         // #7dcfff
            magenta: Color::Rgb(187, 154, 247),      // #bb9af7
            theme_name: "Tokyo Night (Default)".to_string(),
        }
    }
}

pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
    let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
    let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

impl Theme {
    pub fn load_omarchy() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let candidate_paths = [
            (home.join(".local/state/omarchy/current/theme/colors.toml"), "Omarchy Live Theme"),
            (home.join(".config/omarchy/current/theme/colors.toml"), "Omarchy Config Theme"),
            (home.join(".config/ytm-rs/theme.toml"), "Custom User Theme"),
            (home.join(".config/ytm-rs/colors.toml"), "Custom User Theme"),
        ];

        for (path, default_label) in &candidate_paths {
            if path.exists() {
                if let Ok(mut theme) = Self::from_file(path) {
                    if theme.theme_name == "Omarchy Live Theme" {
                        theme.theme_name = default_label.to_string();
                    }
                    return theme;
                }
            }
        }

        Self::default()
    }

    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let raw: RawThemeColors = toml::from_str(&content)?;

        let mut theme = Self::default();
        if let Some(c) = raw.accent.as_deref().and_then(parse_hex_color) {
            theme.accent = c;
        }
        if let Some(c) = raw.background.as_deref().and_then(parse_hex_color) {
            theme.background = c;
        }
        if let Some(c) = raw.dark_background.as_deref().and_then(parse_hex_color) {
            theme.dark_background = c;
        }
        if let Some(c) = raw.foreground.as_deref().and_then(parse_hex_color) {
            theme.foreground = c;
        }
        if let Some(c) = raw.muted.as_deref().and_then(parse_hex_color) {
            theme.muted = c;
        }
        if let Some(c) = raw.selection.as_deref().and_then(parse_hex_color) {
            theme.selection = c;
        }
        if let Some(c) = raw.red.as_deref().and_then(parse_hex_color) {
            theme.red = c;
        }
        if let Some(c) = raw.green.as_deref().and_then(parse_hex_color) {
            theme.green = c;
        }
        if let Some(c) = raw.yellow.as_deref().and_then(parse_hex_color) {
            theme.yellow = c;
        }
        if let Some(c) = raw.cyan.as_deref().and_then(parse_hex_color) {
            theme.cyan = c;
        }
        if let Some(c) = raw.magenta.as_deref().and_then(parse_hex_color) {
            theme.magenta = c;
        }

        if let Some(name) = raw.name {
            theme.theme_name = name;
        } else {
            theme.theme_name = "Omarchy Live Theme".to_string();
        }
        Ok(theme)
    }

    // Styling helpers for Ratatui widgets
    pub fn default_block_style(&self) -> Style {
        Style::default().fg(self.foreground)
    }

    pub fn focused_border_style(&self) -> Style {
        Style::default().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    pub fn unfocused_border_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn selected_row_style(&self) -> Style {
        Style::default()
            .bg(self.selection)
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn title_style(&self) -> Style {
        Style::default().fg(self.accent).add_modifier(Modifier::BOLD)
    }

    pub fn text_style(&self) -> Style {
        Style::default().fg(self.foreground)
    }

    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn playing_indicator_style(&self) -> Style {
        Style::default().fg(self.green).add_modifier(Modifier::BOLD)
    }

    pub fn liked_style(&self) -> Style {
        Style::default().fg(self.red).add_modifier(Modifier::BOLD)
    }
}

pub struct ThemeWatcher {
    _watcher: RecommendedWatcher,
    pub rx: Receiver<()>,
}

impl ThemeWatcher {
    pub fn new() -> Option<Self> {
        let home = dirs::home_dir()?;
        let omarchy_dir = home.join(".local/state/omarchy/current");
        let ytm_dir = home.join(".config/ytm-rs");

        let target_dir = if omarchy_dir.exists() {
            omarchy_dir
        } else if ytm_dir.exists() {
            ytm_dir
        } else {
            return None;
        };

        let (notify_tx, notify_rx) = channel();
        let (event_tx, event_rx) = channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = notify_tx.send(event);
                }
            },
            Config::default(),
        ).ok()?;

        let _ = watcher.watch(&target_dir, RecursiveMode::Recursive);

        std::thread::spawn(move || {
            while let Ok(_event) = notify_rx.recv() {
                // Debounce simple bursts
                std::thread::sleep(std::time::Duration::from_millis(100));
                while notify_rx.try_recv().is_ok() {}
                let _ = event_tx.send(());
            }
        });

        Some(Self {
            _watcher: watcher,
            rx: event_rx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(parse_hex_color("#8FBC8F"), Some(Color::Rgb(143, 188, 143)));
        assert_eq!(parse_hex_color("1a2125"), Some(Color::Rgb(26, 33, 37)));
        assert_eq!(parse_hex_color("invalid"), None);
    }

    #[test]
    fn test_theme_fallback() {
        let theme = Theme::default();
        assert_eq!(theme.accent, Color::Rgb(122, 162, 247));
    }

    #[test]
    fn test_custom_theme_from_str() {
        let toml_data = r##"
            name = "Nord Test"
            accent = "#88c0d0"
            background = "#2e3440"
        "##;
        let raw: RawThemeColors = toml::from_str(toml_data).unwrap();
        assert_eq!(raw.name, Some("Nord Test".to_string()));
        assert_eq!(parse_hex_color(raw.accent.unwrap().as_str()), Some(Color::Rgb(136, 192, 208)));
    }
}
