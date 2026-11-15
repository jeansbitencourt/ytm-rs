use crate::config::AppConfig;
use crate::i18n::{I18n, LanguagePreference};
use anyhow::{Context, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn read_waybar_status(status_file: &Path) -> Result<String> {
    let preference = AppConfig::load()
        .map(|config| config.settings.general.language)
        .unwrap_or(LanguagePreference::Auto);
    let i18n = I18n::new(preference);

    if status_file.exists() {
        let content = std::fs::read_to_string(status_file)
            .with_context(|| format!("Failed to read status file: {:?}", status_file))?;
        let Ok(mut status) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Ok(content);
        };
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let stale = status
            .get("updated_at_ms")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|updated| now_ms.saturating_sub(updated) > 6_000);
        if stale {
            if let Some(object) = status.as_object_mut() {
                object.insert("text".to_string(), serde_json::json!("🎵 YTM-RS"));
                object.insert(
                    "tooltip".to_string(),
                    serde_json::json!(format!("YTM-RS: {}", i18n.t("status.idle"))),
                );
                object.insert("running".to_string(), serde_json::json!(false));
                object.insert("daemon_running".to_string(), serde_json::json!(false));
                object.insert("tui_open".to_string(), serde_json::json!(false));
                object.insert("playing".to_string(), serde_json::json!(false));
                object.insert("paused".to_string(), serde_json::json!(false));
                object.insert("buffering".to_string(), serde_json::json!(false));
                object.insert("stopped".to_string(), serde_json::json!(true));
                object.insert("status".to_string(), serde_json::json!("stopped"));
                object.insert("error".to_string(), serde_json::Value::Null);
            }
        }
        Ok(serde_json::to_string(&status)?)
    } else {
        let fallback = serde_json::json!({
            "text": "🎵 YTM-RS",
            "tooltip": format!("YTM-RS: {}", i18n.t("status.idle")),
            "class": "stopped",
            "running": false,
            "daemon_running": false,
            "paused": false,
            "buffering": false,
            "status": "stopped",
            "stopped": true
        });
        Ok(serde_json::to_string(&fallback)?)
    }
}
