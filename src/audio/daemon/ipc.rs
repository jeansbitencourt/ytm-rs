use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::info;

use super::super::mpv_ipc::MpvCommand;
use super::super::{DaemonCommand, DaemonResponse};
use super::AudioDaemon;
use crate::config::AppConfig;

impl AudioDaemon {
    pub(super) async fn handle_client(self: Arc<Self>, stream: UnixStream) -> Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        if let Ok(Some(line)) = lines.next_line().await {
            let cmd: DaemonCommand = serde_json::from_str(&line)?;
            match cmd {
                DaemonCommand::Subscribe => {
                    // Track TUI client connection
                    {
                        let mut count = self.tui_clients.lock().await;
                        *count += 1;
                    }
                    self.write_status_json().await;

                    // Send initial state
                    let initial = self.state.lock().await.clone();
                    let resp = DaemonResponse::State(Box::new(initial));
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    let _ = writer.write_all(json.as_bytes()).await;

                    let mut rx = self.bcast_tx.subscribe();
                    loop {
                        tokio::select! {
                            res = lines.next_line() => {
                                match res {
                                    Ok(Some(_)) => {}
                                    _ => break,
                                }
                            }
                            state = rx.recv() => {
                                match state {
                                    Ok(state) => {
                                        let resp = DaemonResponse::State(Box::new(state));
                                        let mut line = serde_json::to_string(&resp)?;
                                        line.push('\n');
                                        if writer.write_all(line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }

                    // TUI client disconnected
                    {
                        let mut count = self.tui_clients.lock().await;
                        *count = count.saturating_sub(1);
                    }
                    self.write_status_json().await;
                }
                DaemonCommand::GetState => {
                    let state = self.state.lock().await.clone();
                    let resp = DaemonResponse::State(Box::new(state));
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::PlayTrack(track) => {
                    self.play_track(track).await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::QueueTracks(tracks, idx) => {
                    self.queue_tracks(tracks, idx).await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::EnqueueTrack(track) => {
                    self.enqueue_track(track).await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Toggle => {
                    self.toggle().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Play => {
                    self.play().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Pause => {
                    self.pause().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Stop => {
                    self.stop().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Next => {
                    self.next_track().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Prev => {
                    self.prev_track().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Seek(secs) => {
                    let _ = self.send_mpv_command(MpvCommand::Seek(secs)).await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::VolumeDelta(d) => {
                    self.volume_delta(d).await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::SetVolume(v) => {
                    self.set_volume(v).await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::SetVideoMode(enabled) => {
                    self.set_video_mode(enabled).await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::ToggleVideoMode => {
                    self.toggle_video_mode().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::ToggleLike => {
                    self.toggle_like().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::ToggleShuffle => {
                    self.toggle_shuffle().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::PlaybackAuthChanged => {
                    self.playback_auth_changed().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::StartRadio(_) => {
                    // Handled in coordination with innertube
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::StopDaemon => {
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                    let _ = self.shutdown_tx.send(());
                }
            }
        }

        Ok(())
    }
}

pub async fn send_daemon_command(config: &AppConfig, cmd: DaemonCommand) -> Result<DaemonResponse> {
    ensure_daemon_running(config).await?;

    let stream = UnixStream::connect(&config.state_sock)
        .await
        .with_context(|| {
            format!(
                "Failed to connect to daemon socket at {:?}",
                config.state_sock
            )
        })?;

    let (reader, mut writer) = stream.into_split();
    let mut line = serde_json::to_string(&cmd)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();
    if let Some(resp_line) = lines.next_line().await? {
        let resp: DaemonResponse = serde_json::from_str(&resp_line)?;
        Ok(resp)
    } else {
        Err(anyhow::anyhow!("Daemon closed connection without response"))
    }
}

pub async fn ensure_daemon_running(config: &AppConfig) -> Result<()> {
    if config.state_sock.exists() {
        if let Ok(mut stream) = UnixStream::connect(&config.state_sock).await {
            // Test connection
            let mut line = serde_json::to_string(&DaemonCommand::GetState)?;
            line.push('\n');
            if stream.write_all(line.as_bytes()).await.is_ok() {
                return Ok(());
            }
        }
        let _ = std::fs::remove_file(&config.state_sock);
    }

    let current_exe = std::env::current_exe()?;
    info!("Spawning background daemon: {:?}", current_exe);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(current_exe);
        cmd.arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        cmd.spawn()?;
    }

    // Wait for socket to become available
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if config.state_sock.exists() {
            if UnixStream::connect(&config.state_sock).await.is_ok() {
                return Ok(());
            }
        }
    }

    Err(anyhow::anyhow!(
        "Timed out waiting for daemon socket to be created"
    ))
}
