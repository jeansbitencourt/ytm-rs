use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use anyhow::{Context, Result};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

#[derive(Debug, Clone)]
pub enum MpvCommand {
    LoadFile(String),
    SetPause(bool),
    SetVolume(u8),
    Seek(i64),
    Stop,
}

#[derive(Debug, Clone)]
pub enum MpvEvent {
    TimePos(f64),
    Duration(f64),
    Pause(bool),
    Eof,
}

pub struct MpvController {
    cmd_tx: mpsc::Sender<MpvCommand>,
}

impl MpvController {
    pub async fn send(&self, cmd: MpvCommand) -> Result<()> {
        self.cmd_tx.send(cmd).await.map_err(|_| anyhow::anyhow!("MPV worker channel closed"))
    }
}

pub struct MpvManager {
    sock_path: PathBuf,
}

impl MpvManager {
    pub fn new(sock_path: PathBuf) -> Self {
        Self { sock_path }
    }

    pub async fn start(&self, event_tx: mpsc::Sender<MpvEvent>) -> Result<MpvController> {
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
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(event) = val.get("event").and_then(|e| e.as_str()) {
                        match event {
                            "property-change" => {
                                let name = val.get("name").and_then(|n| n.as_str()).unwrap_or("");
                                match name {
                                    "time-pos" => {
                                        if let Some(pos) = val.get("data").and_then(|d| d.as_f64()) {
                                            let _ = ev_tx.send(MpvEvent::TimePos(pos)).await;
                                        }
                                    }
                                    "duration" => {
                                        if let Some(dur) = val.get("data").and_then(|d| d.as_f64()) {
                                            let _ = ev_tx.send(MpvEvent::Duration(dur)).await;
                                        }
                                    }
                                    "pause" => {
                                        if let Some(p) = val.get("data").and_then(|d| d.as_bool()) {
                                            let _ = ev_tx.send(MpvEvent::Pause(p)).await;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "end-file" => {
                                let reason = val.get("reason").and_then(|r| r.as_str()).unwrap_or("");
                                if reason == "eof" {
                                    let _ = ev_tx.send(MpvEvent::Eof).await;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        // Writer task
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                let json_cmd = match cmd {
                    MpvCommand::LoadFile(url) => {
                        json!({"command": ["loadfile", url, "replace"]})
                    }
                    MpvCommand::SetPause(paused) => {
                        json!({"command": ["set_property", "pause", paused]})
                    }
                    MpvCommand::SetVolume(vol) => {
                        json!({"command": ["set_property", "volume", vol]})
                    }
                    MpvCommand::Seek(secs) => {
                        json!({"command": ["seek", secs, "relative"]})
                    }
                    MpvCommand::Stop => {
                        json!({"command": ["stop"]})
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
                    break;
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
