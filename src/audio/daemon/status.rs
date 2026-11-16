use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::PlaybackStatus;
use super::{current_i18n, AudioDaemon};

impl AudioDaemon {
    pub async fn broadcast_and_save(&self) {
        let state = self.state.lock().await.clone();
        let _ = self.bcast_tx.send(state);
        self.write_status_json().await;
    }

    pub async fn write_status_json(&self) {
        let _write_guard = self.status_write_lock.lock().await;
        let state = self.state.lock().await;
        let i18n = current_i18n(&self.config);
        let file_path = &self.config.status_file;
        let tmp_path = file_path.with_extension("tmp");

        let (title, artist, album, thumbnail_url) = if let Some(t) = &state.current_track {
            (
                t.title.clone(),
                t.artist.clone(),
                t.album.clone(),
                t.thumbnail_url.clone(),
            )
        } else {
            (
                i18n.t("player.no_track"),
                "".to_string(),
                "".to_string(),
                "".to_string(),
            )
        };

        let tui_clients = *self.tui_clients.lock().await;
        let is_tui_open = tui_clients > 0;
        let is_playing = state.status == PlaybackStatus::Playing;
        let is_paused = state.status == PlaybackStatus::Paused;
        let is_buffering = state.status == PlaybackStatus::Buffering;
        let is_stopped = state.status == PlaybackStatus::Stopped;
        let status_name = match state.status {
            PlaybackStatus::Playing => "playing",
            PlaybackStatus::Paused => "paused",
            PlaybackStatus::Buffering => "buffering",
            PlaybackStatus::Stopped => "stopped",
        };
        let daemon_running = self.daemon_running.load(Ordering::Acquire);
        let updated_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let controls_hint = i18n.t("widget.controls_hint");
        let open_controls_hint = i18n.t("widget.open_controls_hint");
        let widget_text = serde_json::json!({
            "controls_hint": controls_hint,
            "open_controls_hint": open_controls_hint,
            "ready": i18n.t("widget.ready"),
            "terminal_open": i18n.t("widget.terminal_open"),
            "description": i18n.t("widget.description"),
            "no_track": i18n.t("player.no_track"),
            "playing_progress": i18n.t("widget.playing_progress"),
            "paused_progress": i18n.t("widget.paused_progress"),
            "buffering_progress": i18n.t("widget.buffering_progress"),
            "failed_progress": i18n.t("widget.failed_progress"),
            "window_active": i18n.t("widget.window_active"),
            "idle": i18n.t("widget.idle"),
            "play_button": i18n.t("widget.play_button"),
            "pause_button": i18n.t("widget.pause_button"),
            "buffering_button": i18n.t("widget.buffering_button"),
            "volume": i18n.t("widget.volume"),
            "focus_button": i18n.t("widget.focus_button"),
            "stop_button": i18n.t("widget.stop_button"),
            "offline_tooltip": i18n.t("widget.offline_tooltip"),
        });

        let status_json = serde_json::json!({
            "text": if is_playing {
                format!("▶ {} - {}", artist, title)
            } else if is_paused {
                format!("⏸ {} - {}", artist, title)
            } else if is_buffering {
                format!("⏳ {} - {}", artist, title)
            } else if state.error_message.is_some() {
                format!("⚠ {} - {}", artist, title)
            } else if is_tui_open {
                i18n.t("status.open")
            } else {
                "🎵 YTM-RS".to_string()
            },
            "tooltip": if let Some(error) = &state.error_message {
                i18n.tf(
                    "status.error_tooltip",
                    [
                        ("error", error.clone()),
                        ("controls", controls_hint.clone()),
                    ],
                )
            } else if is_playing {
                let liked = if state.is_liked {
                    i18n.t("status.liked_yes")
                } else {
                    i18n.t("status.liked_no")
                };
                i18n.tf(
                    "status.playing_tooltip",
                    [
                        ("title", title.clone()),
                        ("artist", artist.clone()),
                        ("volume", state.volume.to_string()),
                        ("liked", liked),
                        ("controls", controls_hint.clone()),
                    ],
                )
            } else if is_paused {
                i18n.tf(
                    "status.paused_tooltip",
                    [
                        ("title", title.clone()),
                        ("artist", artist.clone()),
                        ("volume", state.volume.to_string()),
                        ("controls", controls_hint.clone()),
                    ],
                )
            } else if is_buffering {
                i18n.tf(
                    "status.buffering_tooltip",
                    [
                        ("title", title.clone()),
                        ("artist", artist.clone()),
                        ("controls", controls_hint.clone()),
                    ],
                )
            } else if is_tui_open {
                i18n.tf(
                    "status.window_open_tooltip",
                    [("controls", controls_hint.clone())],
                )
            } else {
                i18n.tf(
                    "status.background_tooltip",
                    [("open_controls", open_controls_hint.clone())],
                )
            },
            "widget": widget_text,
            "running": daemon_running,
            "daemon_running": daemon_running,
            "tui_open": is_tui_open,
            "playing": is_playing,
            "paused": is_paused,
            "buffering": is_buffering,
            "stopped": is_stopped,
            "status": status_name,
            "error": state.error_message,
            "updated_at_ms": updated_at_ms,
            "title": title,
            "artist": artist,
            "album": album,
            "cover_url": thumbnail_url,
            "volume": state.volume,
            "liked": state.is_liked,
            "shuffle": state.shuffle_enabled,
            "position": state.position_secs,
            "duration": state.duration_secs,
            "language": i18n.language.code(),
        });

        if let Ok(content) = serde_json::to_string_pretty(&status_json) {
            if let Ok(_) = std::fs::write(&tmp_path, content) {
                let _ = std::fs::rename(&tmp_path, file_path);
            }
        }
    }
}
