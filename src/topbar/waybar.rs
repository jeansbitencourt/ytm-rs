use crate::config::AppConfig;
use crate::i18n::{I18n, LanguagePreference};
use anyhow::{Context, Result};
use std::path::Path;

pub fn read_waybar_status(status_file: &Path) -> Result<String> {
    if status_file.exists() {
        let content = std::fs::read_to_string(status_file)
            .with_context(|| format!("Failed to read status file: {:?}", status_file))?;
        Ok(content)
    } else {
        let preference = AppConfig::load()
            .map(|config| config.settings.general.language)
            .unwrap_or(LanguagePreference::Auto);
        let i18n = I18n::new(preference);
        let fallback = serde_json::json!({
            "text": "🎵 YTM-RS",
            "tooltip": format!("YTM-RS: {}", i18n.t("status.idle")),
            "class": "stopped",
            "running": false,
            "paused": false,
            "stopped": true
        });
        Ok(serde_json::to_string(&fallback)?)
    }
}
