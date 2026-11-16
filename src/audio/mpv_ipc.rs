use super::prefetch::ResolvedMedia;
use anyhow::{Context, Result};
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info};

#[derive(Debug)]
pub enum MpvCommand {
    LoadFile {
        media: ResolvedMedia,
        generation: u64,
        title: String,
    },
    SetPause(bool),
    SetVolume(u8),
    Seek(i64),
    SeekAbsolute(f64),
    Stop {
        generation: u64,
    },
    Quit(oneshot::Sender<()>),
}

#[derive(Debug, Clone)]
pub enum MpvEvent {
    TimePos { generation: u64, position: f64 },
    Duration { generation: u64, duration: f64 },
    Pause { generation: u64, paused: bool },
    PlaybackRestart { generation: u64 },
    Stopped { generation: u64 },
    FileError { generation: u64, message: String },
    Eof { generation: u64 },
    ConnectionLost(String),
}

#[derive(Default)]
struct EventContext {
    pending_loads: VecDeque<u64>,
    active_generation: Option<u64>,
}

pub struct MpvController {
    cmd_tx: mpsc::Sender<MpvCommand>,
}

impl MpvController {
    pub async fn send(
        &self,
        cmd: MpvCommand,
    ) -> std::result::Result<(), mpsc::error::SendError<MpvCommand>> {
        self.cmd_tx.send(cmd).await
    }
}

#[derive(Clone)]
pub struct MpvManager {
    sock_path: PathBuf,
    process: Arc<Mutex<Option<Child>>>,
}

impl MpvManager {
    pub fn new(sock_path: PathBuf) -> Self {
        Self {
            sock_path,
            process: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn start(
        &self,
        event_tx: mpsc::Sender<MpvEvent>,
        play_generation: Arc<Mutex<u64>>,
    ) -> Result<MpvController> {
        let mut stream = None;
        let mut last_error = None;
        for attempt in 0..3 {
            self.ensure_process().await?;

            for _ in 0..30 {
                if self.sock_path.exists() {
                    if let Ok(Ok(mut candidate)) = tokio::time::timeout(
                        Duration::from_millis(250),
                        UnixStream::connect(&self.sock_path),
                    )
                    .await
                    {
                        match verify_mpv_connection(&mut candidate).await {
                            Ok(()) => {
                                stream = Some(candidate);
                                break;
                            }
                            Err(error) => last_error = Some(error.to_string()),
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            if stream.is_some() {
                break;
            }
            if attempt < 2 {
                self.restart_process().await?;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }

        let stream = stream.context(format!(
            "Failed to connect to a responsive MPV IPC socket{}",
            last_error
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ))?;
        restrict_socket_permissions(&self.sock_path)?;
        let (reader, mut writer) = stream.into_split();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<MpvCommand>(32);
        let event_context = Arc::new(StdMutex::new(EventContext::default()));

        // Subscribe to properties
        let init_cmds = [
            json!({"command": ["observe_property", 1, "time-pos"]}),
            json!({"command": ["observe_property", 2, "duration"]}),
            json!({"command": ["observe_property", 3, "pause"]}),
        ];

        for cmd in &init_cmds {
            let mut line = serde_json::to_string(cmd)?;
            line.push('\n');
            writer.write_all(line.as_bytes()).await?;
        }

        // Reader task
        let ev_tx = event_tx.clone();
        let reader_context = Arc::clone(&event_context);
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(event) = val.get("event").and_then(|e| e.as_str()) {
                        match event {
                            "start-file" => {
                                let mut context = reader_context
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                context.active_generation = context.pending_loads.pop_front();
                            }
                            "property-change" => {
                                let name = val.get("name").and_then(|n| n.as_str()).unwrap_or("");
                                let generation = reader_context
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .active_generation;
                                let Some(generation) = generation else {
                                    continue;
                                };
                                match name {
                                    "time-pos" => {
                                        if let Some(pos) = val.get("data").and_then(|d| d.as_f64())
                                        {
                                            let _ = ev_tx
                                                .send(MpvEvent::TimePos {
                                                    generation,
                                                    position: pos,
                                                })
                                                .await;
                                        }
                                    }
                                    "duration" => {
                                        if let Some(dur) = val.get("data").and_then(|d| d.as_f64())
                                        {
                                            let _ = ev_tx
                                                .send(MpvEvent::Duration {
                                                    generation,
                                                    duration: dur,
                                                })
                                                .await;
                                        }
                                    }
                                    "pause" => {
                                        if let Some(p) = val.get("data").and_then(|d| d.as_bool()) {
                                            let _ = ev_tx
                                                .send(MpvEvent::Pause {
                                                    generation,
                                                    paused: p,
                                                })
                                                .await;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "playback-restart" => {
                                let generation = reader_context
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .active_generation;
                                if let Some(generation) = generation {
                                    let _ =
                                        ev_tx.send(MpvEvent::PlaybackRestart { generation }).await;
                                }
                            }
                            "end-file" => {
                                let reason =
                                    val.get("reason").and_then(|r| r.as_str()).unwrap_or("");
                                let generation = reader_context
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .active_generation;
                                let Some(generation) = generation else {
                                    continue;
                                };
                                if reason == "eof" {
                                    let _ = ev_tx.send(MpvEvent::Eof { generation }).await;
                                } else if reason == "stop" {
                                    let _ = ev_tx.send(MpvEvent::Stopped { generation }).await;
                                } else if reason == "error" {
                                    let message = "MPV could not load media".to_string();
                                    let _ = ev_tx
                                        .send(MpvEvent::FileError {
                                            generation,
                                            message,
                                        })
                                        .await;
                                }
                            }
                            _ => {}
                        }
                    } else if let Some(request_id) =
                        val.get("request_id").and_then(|id| id.as_u64())
                    {
                        let result = val.get("error").and_then(|e| e.as_str()).unwrap_or("");
                        if result != "success" {
                            {
                                let mut context = reader_context
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                if let Some(index) = context
                                    .pending_loads
                                    .iter()
                                    .position(|generation| *generation == request_id)
                                {
                                    context.pending_loads.remove(index);
                                }
                            }
                            let _ = ev_tx
                                .send(MpvEvent::FileError {
                                    generation: request_id,
                                    message: "MPV rejected the load command".to_string(),
                                })
                                .await;
                        }
                    }
                }
            }
            let _ = ev_tx
                .send(MpvEvent::ConnectionLost(
                    "MPV IPC socket closed".to_string(),
                ))
                .await;
        });

        // Writer task
        let writer_context = Arc::clone(&event_context);
        let writer_generation = Arc::clone(&play_generation);
        let writer_event_tx = event_tx;
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                if let MpvCommand::LoadFile {
                    media,
                    generation,
                    title,
                } = cmd
                {
                    // Serialize generation changes with the actual IPC write. A stale
                    // resolver may have queued a command before a newer selection.
                    let current_generation = writer_generation.lock().await;
                    if *current_generation != generation {
                        debug!(
                            "Discarding stale MPV load request for generation {} (current {})",
                            generation, *current_generation
                        );
                        continue;
                    }
                    writer_context
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .pending_loads
                        .push_back(generation);
                    let headers = encode_http_header_fields(&media.headers);
                    let mut per_file_options = serde_json::Map::new();
                    if !headers.is_empty() {
                        per_file_options.insert("http-header-fields".to_string(), json!(headers));
                    }
                    if let Some(audio_url) = media.external_audio_url.as_deref() {
                        // `audio-files` is a path-list option on Unix. Escape
                        // colons in URLs so `https:` is not parsed as a second file.
                        per_file_options.insert(
                            "audio-files".to_string(),
                            json!(escape_mpv_path_list_entry(audio_url)),
                        );
                    }
                    if media.auth_source.is_enabled() {
                        // Disable libcurl's transparent retries for an authenticated stream.
                        per_file_options.insert("curl-max-retries".to_string(), json!("0"));
                    }
                    per_file_options.insert("force-media-title".to_string(), json!(title));
                    let load_command = if per_file_options.is_empty() {
                        json!({
                            "command": ["loadfile", media.url, "replace"],
                            "request_id": generation
                        })
                    } else {
                        json!({
                            "command": [
                                "loadfile",
                                media.url,
                                "replace",
                                -1,
                                per_file_options
                            ],
                            "request_id": generation
                        })
                    };
                    let cmds = [
                        load_command,
                        json!({"command": ["set_property", "pause", false]}),
                    ];
                    let mut write_error = None;
                    for c in &cmds {
                        if let Ok(mut l) = serde_json::to_string(c) {
                            l.push('\n');
                            if let Err(e) = writer.write_all(l.as_bytes()).await {
                                error!("Failed to write to MPV socket: {}", e);
                                write_error = Some(e.to_string());
                                break;
                            }
                        }
                    }
                    drop(current_generation);
                    if let Some(message) = write_error {
                        let _ = writer_event_tx
                            .send(MpvEvent::ConnectionLost(message))
                            .await;
                        break;
                    }
                    continue;
                }

                if let MpvCommand::Stop { generation } = cmd {
                    let current_generation = writer_generation.lock().await;
                    if *current_generation != generation {
                        continue;
                    }
                    let mut line = serde_json::to_string(&json!({"command": ["stop"]}))
                        .expect("MPV stop command is valid JSON");
                    line.push('\n');
                    let write_error = writer.write_all(line.as_bytes()).await.err();
                    drop(current_generation);
                    if let Some(e) = write_error {
                        error!("Failed to write to MPV socket: {}", e);
                        let _ = writer_event_tx
                            .send(MpvEvent::ConnectionLost(e.to_string()))
                            .await;
                        break;
                    }
                    continue;
                }

                let (json_cmd, quit_ack) = match cmd {
                    MpvCommand::LoadFile { .. } => unreachable!(),
                    MpvCommand::Stop { .. } => unreachable!(),
                    MpvCommand::Quit(ack) => (json!({"command": ["quit"]}), Some(ack)),
                    MpvCommand::SetPause(paused) => {
                        (json!({"command": ["set_property", "pause", paused]}), None)
                    }
                    MpvCommand::SetVolume(vol) => {
                        (json!({"command": ["set_property", "volume", vol]}), None)
                    }
                    MpvCommand::Seek(secs) => {
                        (json!({"command": ["seek", secs, "relative"]}), None)
                    }
                    MpvCommand::SeekAbsolute(secs) => (
                        json!({"command": ["seek", secs.max(0.0), "absolute"]}),
                        None,
                    ),
                };

                let mut line = match serde_json::to_string(&json_cmd) {
                    Ok(l) => l,
                    Err(e) => {
                        error!("Failed to serialize mpv command: {}", e);
                        continue;
                    }
                };
                line.push('\n');
                if let Err(e) = writer.write_all(line.as_bytes()).await {
                    error!("Failed to write to MPV socket: {}", e);
                    drop(quit_ack);
                    let _ = writer_event_tx
                        .send(MpvEvent::ConnectionLost(e.to_string()))
                        .await;
                    break;
                }
                if let Some(ack) = quit_ack {
                    let _ = ack.send(());
                }
            }
        });

        Ok(MpvController { cmd_tx })
    }

    async fn ensure_process(&self) -> Result<()> {
        let mut process = self.process.lock().await;
        let is_running = match process.as_mut() {
            Some(child) => child.try_wait()?.is_none(),
            None => false,
        };
        if is_running {
            return Ok(());
        }

        if process.is_some() {
            *process = None;
            let _ = std::fs::remove_file(&self.sock_path);
        }

        if self.sock_path.exists() {
            if UnixStream::connect(&self.sock_path).await.is_ok() {
                debug!("Adopting the existing MPV IPC socket");
                return Ok(());
            }
            // Remove stale socket
            let _ = std::fs::remove_file(&self.sock_path);
        }

        let sock_arg = format!("--input-ipc-server={}", self.sock_path.display());
        info!("Spawning MPV playback process: {}", sock_arg);

        let child = Command::new("mpv")
            .arg("--no-ytdl")
            .arg("--idle=yes")
            .arg(&sock_arg)
            .arg("--audio-client-name=ytm-rs")
            .arg("--volume=75")
            .arg("--gapless-audio=yes")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn mpv process. Please verify that mpv is installed.")?;
        *process = Some(child);

        Ok(())
    }

    async fn restart_process(&self) -> Result<()> {
        let mut process = self.process.lock().await;
        if let Some(child) = process.as_mut() {
            if child.try_wait()?.is_none() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }
        *process = None;
        if self.sock_path.exists() {
            let _ = std::fs::remove_file(&self.sock_path);
        }
        Ok(())
    }
}

async fn verify_mpv_connection(stream: &mut UnixStream) -> Result<()> {
    const PROBE_REQUEST_ID: u64 = 987_654_321;
    let mut request = serde_json::to_vec(&json!({
        "command": ["get_property", "mpv-version"],
        "request_id": PROBE_REQUEST_ID
    }))?;
    request.push(b'\n');
    stream.write_all(&request).await?;

    let response = tokio::time::timeout(Duration::from_secs(2), async {
        let mut line = Vec::new();
        loop {
            let byte = stream.read_u8().await?;
            if byte == b'\n' {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) {
                    if value.get("request_id").and_then(serde_json::Value::as_u64)
                        == Some(PROBE_REQUEST_ID)
                    {
                        return Ok::<_, anyhow::Error>(value);
                    }
                }
                line.clear();
            } else if line.len() < 65_536 {
                line.push(byte);
            } else {
                anyhow::bail!("MPV returned an oversized IPC message");
            }
        }
    })
    .await
    .context("Timed out while checking the MPV IPC socket")??;

    if response.get("error").and_then(serde_json::Value::as_str) != Some("success") {
        anyhow::bail!("MPV IPC health check failed: {response}");
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_socket_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| "Could not restrict MPV IPC socket permissions")
}

fn encode_http_header_fields(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(name, value)| {
            let mut encoded_value = String::with_capacity(value.len());
            for character in value.chars() {
                if character == ',' || character == '\\' {
                    encoded_value.push('\\');
                }
                encoded_value.push(character);
            }
            format!("{name}: {encoded_value}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn escape_mpv_path_list_entry(path: &str) -> String {
    path.replace('\\', "\\\\").replace(':', "\\:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_mpvs_per_file_header_list() {
        let headers = vec![
            (
                "Referer".to_string(),
                "https://music.youtube.com/".to_string(),
            ),
            ("Cookie".to_string(), "one=1,two=2\\three".to_string()),
        ];
        assert_eq!(
            encode_http_header_fields(&headers),
            "Referer: https://music.youtube.com/,Cookie: one=1\\,two=2\\\\three"
        );
    }
}

#[cfg(not(unix))]
fn restrict_socket_permissions(_path: &std::path::Path) -> Result<()> {
    Ok(())
}
