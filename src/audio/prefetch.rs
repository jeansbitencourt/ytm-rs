use crate::cache::Database;
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

pub async fn resolve_stream_url(video_id: &str, db: &Arc<Mutex<Database>>) -> Result<String> {
    // 1. Check local SQLite cache first
    {
        let db_guard = db.lock().await;
        if let Ok(Some(cached_url)) = db_guard.get_stream_url(video_id) {
            debug!("Stream URL cache hit for video_id: {}", video_id);
            return Ok(cached_url);
        }
    }

    info!(
        "Extracting Opus stream URL via yt-dlp for video_id: {}",
        video_id
    );
    let watch_url = format!("https://music.youtube.com/watch?v={}", video_id);

    let output = Command::new("yt-dlp")
        .arg("-f")
        .arg("251/bestaudio[acodec=opus]/bestaudio")
        .arg("-g")
        .arg("--no-playlist")
        .arg(&watch_url)
        .output()
        .await
        .context("Failed to run yt-dlp. Make sure yt-dlp is installed and in PATH.")?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("yt-dlp failed: {}", err.trim()));
    }

    let url = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    if url.is_empty() {
        return Err(anyhow::anyhow!("yt-dlp returned an empty stream URL"));
    }

    // Cache for 4 hours (14400 seconds)
    {
        let db_guard = db.lock().await;
        let _ = db_guard.save_stream_url(video_id, &url, 14400);
    }

    Ok(url)
}

pub fn spawn_prefetch(video_id: String, db: Arc<Mutex<Database>>) {
    tokio::spawn(async move {
        debug!("Starting async prefetch worker for video_id: {}", video_id);
        match resolve_stream_url(&video_id, &db).await {
            Ok(_) => debug!("Prefetch completed successfully for: {}", video_id),
            Err(e) => error!("Prefetch failed for {}: {}", video_id, e),
        }
    });
}
