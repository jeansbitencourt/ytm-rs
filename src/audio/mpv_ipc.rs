use anyhow::{Context, Result};
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info};

#[derive(Debug)]
pub enum MpvCommand {
    LoadFile { url: String, generation: u64 },
    SetPause(bool),
    SetVolume(u8),
    Seek(i64),
    Stop { generation: u64 },
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
    pub async fn send(&self, cmd: MpvCommand) -> Result<()> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| anyhow::anyhow!("MPV worker channel closed"))
    }
}

pub struct MpvManager {
    sock_path: PathBuf,
}

impl MpvManager {
    pub fn new(sock_path: PathBuf) -> Self {
        Self { sock_path }
    }

    pub async fn start(
        &self,
        event_tx: mpsc::Sender<MpvEvent>,
        play_generation: Arc<Mutex<u64>>,
    ) -> Result<MpvController> {
        self.ensure_process().await?;

        // Give mpv a moment to create the socket
        let mut stream = None;
        for _ in 0..20 {
            if self.sock_path.exists() {
                if let Ok(s) = UnixStream::connect(&self.sock_path).await {
                    stream = Some(s);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let stream = stream.context("Failed to connect to MPV IPC socket")?;
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
                                    let message = val
                                        .get("file_error")
                                        .and_then(|error| error.as_str())
                                        .unwrap_or("MPV could not load the stream")
                                        .to_string();
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
                                    message: format!("MPV rejected the load command: {result}"),
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
                if let MpvCommand::LoadFile { url, generation } = cmd {
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
                    let cmds = [
                        json!({
                            "command": ["loadfile", url, "replace"],
                            "request_id": generation
                        }),
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
        if self.sock_path.exists() {
            if UnixStream::connect(&self.sock_path).await.is_ok() {
                debug!("MPV process already running and responsive");
                return Ok(());
            }
            // Remove stale socket
            let _ = std::fs::remove_file(&self.sock_path);
        }

        let sock_arg = format!("--input-ipc-server={}", self.sock_path.display());
        info!("Spawning MPV headless process: {}", sock_arg);

        Command::new("mpv")
            .arg("--no-video")
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

        Ok(())
    }
}
