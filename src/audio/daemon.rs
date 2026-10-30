use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, error, info};

use crate::cache::Database;
use crate::config::AppConfig;
use crate::innertube::Track;
use super::mpv_ipc::{MpvCommand, MpvEvent, MpvManager};
use super::prefetch::{resolve_stream_url, spawn_prefetch};
use super::{DaemonCommand, DaemonResponse, PlaybackStatus, PlayerState};

pub struct AudioDaemon {
    config: AppConfig,
    state: Arc<Mutex<PlayerState>>,
    db: Arc<Mutex<Database>>,
    bcast_tx: broadcast::Sender<PlayerState>,
    mpv_tx: mpsc::Sender<MpvCommand>,
    prefetch_triggered: Arc<Mutex<bool>>,
    tui_clients: Arc<Mutex<usize>>,
}

impl AudioDaemon {
    pub async fn new(config: AppConfig) -> Result<(Self, mpsc::Receiver<MpvEvent>)> {
        let db = Database::open(&config.db_file)?;
        let db = Arc::new(Mutex::new(db));

        let (bcast_tx, _) = broadcast::channel(128);
        let (mpv_event_tx, mpv_event_rx) = mpsc::channel(64);

        let mpv_manager = MpvManager::new(config.mpv_sock.clone());
        let mpv_controller = mpv_manager.start(mpv_event_tx).await?;

        // Internal channel from daemon methods to mpv controller
        let (mpv_cmd_tx, mut mpv_cmd_rx) = mpsc::channel::<MpvCommand>(32);
        tokio::spawn(async move {
            while let Some(cmd) = mpv_cmd_rx.recv().await {
                let _ = mpv_controller.send(cmd).await;
            }
        });

        let state = Arc::new(Mutex::new(PlayerState::default()));

        Ok((
            Self {
                config,
                state,
                db,
                bcast_tx,
                mpv_tx: mpv_cmd_tx,
                prefetch_triggered: Arc::new(Mutex::new(false)),
                tui_clients: Arc::new(Mutex::new(0)),
            },
            mpv_event_rx,
        ))
    }

    pub async fn run(self: Arc<Self>, mut mpv_events: mpsc::Receiver<MpvEvent>) -> Result<()> {
        let sock_path = &self.config.state_sock;
        if sock_path.exists() {
            let _ = std::fs::remove_file(sock_path);
        }

        let listener = UnixListener::bind(sock_path)
            .with_context(|| format!("Failed to bind state socket at {:?}", sock_path))?;
        info!("Audio daemon listening on {:?}", sock_path);

        // Write initial status.json
        self.write_status_json().await;

        // MPRIS2 Integration
        let (mpris_cmd_tx, mut mpris_cmd_rx) = mpsc::channel(32);
        let daemon_for_mpris = Arc::clone(&self);
        tokio::spawn(async move {
            while let Some(cmd) = mpris_cmd_rx.recv().await {
                match cmd {
                    DaemonCommand::Toggle => daemon_for_mpris.toggle().await,
                    DaemonCommand::Play => {
                        let mut state = daemon_for_mpris.state.lock().await;
                        if state.status == PlaybackStatus::Paused {
                            state.status = PlaybackStatus::Playing;
                            let _ = daemon_for_mpris.mpv_tx.send(MpvCommand::SetPause(false)).await;
                        }
                        drop(state);
                        daemon_for_mpris.broadcast_and_save().await;
                    }
                    DaemonCommand::Pause => {
                        let mut state = daemon_for_mpris.state.lock().await;
                        if state.status == PlaybackStatus::Playing {
                            state.status = PlaybackStatus::Paused;
                            let _ = daemon_for_mpris.mpv_tx.send(MpvCommand::SetPause(true)).await;
                        }
                        drop(state);
                        daemon_for_mpris.broadcast_and_save().await;
                    }
                    DaemonCommand::Next => daemon_for_mpris.next_track().await,
                    DaemonCommand::Prev => daemon_for_mpris.prev_track().await,
                    DaemonCommand::Stop => {
                        let mut state = daemon_for_mpris.state.lock().await;
                        state.status = PlaybackStatus::Stopped;
                        drop(state);
                        let _ = daemon_for_mpris.mpv_tx.send(MpvCommand::Stop).await;
                        daemon_for_mpris.broadcast_and_save().await;
                    }
                    _ => {}
                }
            }
        });

        if let Ok(mut mpris) = crate::topbar::mpris::MprisServer::new(mpris_cmd_tx) {
            let mut state_rx = self.bcast_tx.subscribe();
            tokio::spawn(async move {
                while let Ok(state) = state_rx.recv().await {
                    mpris.update(&state);
                }
            });
        }

        // MPV event handler loop
        let daemon_clone = Arc::clone(&self);
        tokio::spawn(async move {
            while let Some(event) = mpv_events.recv().await {
                daemon_clone.handle_mpv_event(event).await;
            }
        });

        // Socket listener loop
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let daemon = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(e) = daemon.handle_client(stream).await {
                            debug!("Client connection ended: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Error accepting client connection: {}", e);
                }
            }
        }
    }

    async fn handle_mpv_event(&self, event: MpvEvent) {
        let mut state = self.state.lock().await;
        match event {
            MpvEvent::TimePos(pos) => {
                state.position_secs = pos;
                // Check 80% prefetch condition
                if state.duration_secs > 0.0 {
                    let progress = state.position_secs / state.duration_secs;
                    if progress >= 0.80 {
                        let mut triggered = self.prefetch_triggered.lock().await;
                        if !*triggered {
                            *triggered = true;
                            let next_idx = state.queue_index + 1;
                            if next_idx < state.queue.len() {
                                let next_track = state.queue[next_idx].clone();
                                spawn_prefetch(next_track.video_id, Arc::clone(&self.db));
                            }
                        }
                    }
                }
            }
            MpvEvent::Duration(dur) => {
                state.duration_secs = dur;
            }
            MpvEvent::Pause(is_paused) => {
                if state.current_track.is_some() {
                    state.status = if is_paused {
                        PlaybackStatus::Paused
                    } else {
                        PlaybackStatus::Playing
                    };
                }
            }
            MpvEvent::Eof => {
                drop(state);
                self.next_track().await;
                return;
            }
        }

        let state_clone = state.clone();
        drop(state);
        let _ = self.bcast_tx.send(state_clone);
        self.write_status_json().await;
    }

    pub async fn play_track(&self, track: Track) {
        {
            let mut state = self.state.lock().await;
            state.status = PlaybackStatus::Buffering;
            state.position_secs = 0.0;
            state.duration_secs = track.duration_secs as f64;
            state.current_track = Some(track.clone());

            let db = self.db.lock().await;
            state.is_liked = db.is_track_liked(&track.video_id).unwrap_or(false);
            let _ = db.add_history(&track);
        }
        *self.prefetch_triggered.lock().await = false;
        self.broadcast_and_save().await;

        let db = Arc::clone(&self.db);
        let video_id = track.video_id.clone();
        let mpv_tx = self.mpv_tx.clone();

        match resolve_stream_url(&video_id, &db).await {
            Ok(url) => {
                let _ = mpv_tx.send(MpvCommand::LoadFile(url)).await;
                let mut state = self.state.lock().await;
                state.status = PlaybackStatus::Playing;
            }
            Err(e) => {
                error!("Failed to resolve stream URL for {}: {}", video_id, e);
                let mut state = self.state.lock().await;
                state.status = PlaybackStatus::Stopped;
            }
        }
        self.broadcast_and_save().await;
    }

    pub async fn queue_tracks(&self, tracks: Vec<Track>, start_idx: usize) {
        if tracks.is_empty() {
            return;
        }
        let selected_track = tracks.get(start_idx).cloned();
        {
            let mut state = self.state.lock().await;
            state.queue = tracks;
            state.queue_index = start_idx;
        }

        if let Some(track) = selected_track {
            self.play_track(track).await;
        }
    }

    pub async fn enqueue_track(&self, track: Track) {
        let should_play = {
            let mut state = self.state.lock().await;
            state.queue.push(track.clone());
            state.current_track.is_none() || state.status == PlaybackStatus::Stopped
        };

        if should_play {
            let idx = {
                let state = self.state.lock().await;
                state.queue.len() - 1
            };
            {
                let mut state = self.state.lock().await;
                state.queue_index = idx;
            }
            self.play_track(track).await;
        } else {
            self.broadcast_and_save().await;
        }
    }

    pub async fn next_track(&self) {
        let next_track = {
            let mut state = self.state.lock().await;
            if state.queue.is_empty() {
                None
            } else if state.queue_index + 1 < state.queue.len() {
                state.queue_index += 1;
                Some(state.queue[state.queue_index].clone())
            } else {
                state.status = PlaybackStatus::Stopped;
                None
            }
        };

        if let Some(track) = next_track {
            self.play_track(track).await;
        } else {
            let _ = self.mpv_tx.send(MpvCommand::Stop).await;
            self.broadcast_and_save().await;
        }
    }

    pub async fn prev_track(&self) {
        let prev_track = {
            let mut state = self.state.lock().await;
            if state.queue.is_empty() {
                None
            } else if state.position_secs > 3.0 {
                // Restart current song
                Some(state.queue[state.queue_index].clone())
            } else if state.queue_index > 0 {
                state.queue_index -= 1;
                Some(state.queue[state.queue_index].clone())
            } else {
                Some(state.queue[0].clone())
            }
        };

        if let Some(track) = prev_track {
            self.play_track(track).await;
        }
    }

    pub async fn toggle(&self) {
        let mut state = self.state.lock().await;
        match state.status {
            PlaybackStatus::Playing => {
                state.status = PlaybackStatus::Paused;
                let _ = self.mpv_tx.send(MpvCommand::SetPause(true)).await;
            }
            PlaybackStatus::Paused => {
                state.status = PlaybackStatus::Playing;
                let _ = self.mpv_tx.send(MpvCommand::SetPause(false)).await;
            }
            PlaybackStatus::Stopped => {
                if let Some(track) = state.queue.get(state.queue_index).cloned() {
                    drop(state);
                    self.play_track(track).await;
                    return;
                }
            }
            PlaybackStatus::Buffering => {}
        }
        drop(state);
        self.broadcast_and_save().await;
    }

    pub async fn set_volume(&self, vol: u8) {
        let clamped = vol.min(100);
        {
            let mut state = self.state.lock().await;
            state.volume = clamped;
        }
        let _ = self.mpv_tx.send(MpvCommand::SetVolume(clamped)).await;
        self.broadcast_and_save().await;
    }

    pub async fn volume_delta(&self, delta: i32) {
        let new_vol = {
            let state = self.state.lock().await;
            let current = state.volume as i32;
            (current + delta).clamp(0, 100) as u8
        };
        self.set_volume(new_vol).await;
    }

    pub async fn toggle_like(&self) {
        let (video_id, current_track, is_liked) = {
            let mut state = self.state.lock().await;
            let new_liked = !state.is_liked;
            state.is_liked = new_liked;
            if let Some(track) = &mut state.current_track {
                track.is_liked = new_liked;
                (track.video_id.clone(), track.clone(), new_liked)
            } else {
                return;
            }
        };

        // Update local database
        {
            let db = self.db.lock().await;
            if is_liked {
                let _ = db.add_liked_track(&current_track);
            } else {
                let _ = db.remove_liked_track(&video_id);
            }
        }

        // Send desktop notification
        let msg = if is_liked {
            format!("♥ Adicionada aos favoritos:\n{} - {}", current_track.title, current_track.artist)
        } else {
            format!("Removida dos favoritos:\n{} - {}", current_track.title, current_track.artist)
        };

        tokio::spawn(async move {
            let _ = tokio::process::Command::new("notify-send")
                .arg("-a")
                .arg("YTM-RS")
                .arg("-i")
                .arg("ytm-rs")
                .arg("YouTube Music")
                .arg(msg)
                .spawn();
        });

        self.broadcast_and_save().await;
    }

    pub async fn broadcast_and_save(&self) {
        let state = self.state.lock().await.clone();
        let _ = self.bcast_tx.send(state);
        self.write_status_json().await;
    }

    pub async fn write_status_json(&self) {
        let state = self.state.lock().await;
        let file_path = &self.config.status_file;
        let tmp_path = file_path.with_extension("tmp");

        let (title, artist, album) = if let Some(t) = &state.current_track {
            (t.title.clone(), t.artist.clone(), t.album.clone())
        } else {
            ("Nenhuma reprodução".to_string(), "".to_string(), "".to_string())
        };

        let tui_clients = *self.tui_clients.lock().await;
        let is_tui_open = tui_clients > 0;
        let is_playing = state.status == PlaybackStatus::Playing;
        let is_paused = state.status == PlaybackStatus::Paused;
        let is_stopped = state.status == PlaybackStatus::Stopped;

        let status_json = serde_json::json!({
            "text": if is_playing {
                format!("▶ {} - {}", artist, title)
            } else if is_paused {
                format!("⏸ {} - {}", artist, title)
            } else if is_tui_open {
                "🎵 YTM-RS (Aberto)".to_string()
            } else {
                "🎵 YTM-RS".to_string()
            },
            "tooltip": if is_playing {
                format!("Tocando: {} - {}\nVolume: {}%\nCurtida: {}", title, artist, state.volume, if state.is_liked { "Sim ♥" } else { "Não" })
            } else if is_paused {
                format!("Pausado: {} - {}\nVolume: {}%", title, artist, state.volume)
            } else if is_tui_open {
                "YTM-RS: Janela aberta (Aguardando reprodução)\nBotão esquerdo: Focar janela\nBotão direito: Controles".to_string()
            } else {
                "YTM-RS: Segundo plano\nBotão esquerdo: Abrir janela\nBotão direito: Controles".to_string()
            },
            "running": true,
            "daemon_running": true,
            "tui_open": is_tui_open,
            "playing": is_playing,
            "paused": is_paused,
            "stopped": is_stopped,
            "title": title,
            "artist": artist,
            "album": album,
            "volume": state.volume,
            "liked": state.is_liked,
            "position": state.position_secs,
            "duration": state.duration_secs,
        });

        if let Ok(content) = serde_json::to_string_pretty(&status_json) {
            if let Ok(_) = std::fs::write(&tmp_path, content) {
                let _ = std::fs::rename(&tmp_path, file_path);
            }
        }
    }

    async fn handle_client(self: Arc<Self>, stream: UnixStream) -> Result<()> {
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
                    let mut state = self.state.lock().await;
                    if state.status == PlaybackStatus::Paused {
                        state.status = PlaybackStatus::Playing;
                        let _ = self.mpv_tx.send(MpvCommand::SetPause(false)).await;
                    }
                    drop(state);
                    self.broadcast_and_save().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Pause => {
                    let mut state = self.state.lock().await;
                    if state.status == PlaybackStatus::Playing {
                        state.status = PlaybackStatus::Paused;
                        let _ = self.mpv_tx.send(MpvCommand::SetPause(true)).await;
                    }
                    drop(state);
                    self.broadcast_and_save().await;
                    let resp = DaemonResponse::Ok;
                    let mut json = serde_json::to_string(&resp)?;
                    json.push('\n');
                    writer.write_all(json.as_bytes()).await?;
                }
                DaemonCommand::Stop => {
                    let mut state = self.state.lock().await;
                    state.status = PlaybackStatus::Stopped;
                    drop(state);
                    let _ = self.mpv_tx.send(MpvCommand::Stop).await;
                    self.broadcast_and_save().await;
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
                    let _ = self.mpv_tx.send(MpvCommand::Seek(secs)).await;
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
                DaemonCommand::ToggleLike => {
                    self.toggle_like().await;
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
                    let _ = self.mpv_tx.send(MpvCommand::Stop).await;
                    std::process::exit(0);
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
        .with_context(|| format!("Failed to connect to daemon socket at {:?}", config.state_sock))?;

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

    Err(anyhow::anyhow!("Timed out waiting for daemon socket to be created"))
}
