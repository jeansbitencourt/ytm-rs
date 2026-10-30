use std::path::Path;
use anyhow::{Context, Result};

pub fn read_waybar_status(status_file: &Path) -> Result<String> {
    if status_file.exists() {
        let content = std::fs::read_to_string(status_file)
            .with_context(|| format!("Failed to read status file: {:?}", status_file))?;
        Ok(content)
    } else {
        let fallback = serde_json::json!({
            "text": "🎵 YTM-RS",
            "tooltip": "YTM-RS: Inativo",
            "class": "stopped",
            "running": false,
            "paused": false,
            "stopped": true
        });
        Ok(serde_json::to_string(&fallback)?)
    }
}
