use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, error, info, warn};

use super::mpv_ipc::{MpvCommand, MpvEvent, MpvManager};
use super::prefetch::{resolve_stream_url, spawn_prefetch};
use super::{DaemonCommand, DaemonResponse, PlaybackStatus, PlayerState};
use crate::cache::Database;
use crate::config::AppConfig;
use crate::i18n::I18n;
use crate::innertube::Track;

fn current_i18n(config: &AppConfig) -> I18n {
    let preference = AppConfig::load()
        .map(|settings| settings.settings.general.language)
        .unwrap_or(config.settings.general.language);
    I18n::new(preference)
}

struct SimpleRng(u64);

impl SimpleRng {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(123456789);
        let pid = std::process::id() as u64;
        let seed = nanos ^ (pid << 32) ^ 0x9e3779b97f4a7c15;
        Self(if seed == 0 { 0x853c49e6748fea9b } else { seed })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn gen_range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            return 0;
        }
        (self.next_u64() % (upper as u64)) as usize
    }
}

pub fn shuffle_tracks_preserving_current(tracks: &mut Vec<Track>, current_idx: usize) {
    if tracks.is_empty() {
        return;
    }
    if current_idx < tracks.len() && current_idx != 0 {
        tracks.swap(0, current_idx);
    }
    if tracks.len() > 2 {
        let mut rng = SimpleRng::new();
        for i in (2..tracks.len()).rev() {
            let j = 1 + rng.gen_range(i);
            tracks.swap(i, j);
        }
    }
}

pub struct AudioDaemon {
    config: AppConfig,
    state: Arc<Mutex<PlayerState>>,
    db: Arc<Mutex<Database>>,
    bcast_tx: broadcast::Sender<PlayerState>,
    mpv_tx: mpsc::Sender<MpvCommand>,
    prefetch_triggered: Arc<Mutex<bool>>,
    tui_clients: Arc<Mutex<usize>>,
    play_generation: Arc<Mutex<u64>>,
    original_queue: Arc<Mutex<Vec<Track>>>,
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
                play_generation: Arc::new(Mutex::new(0)),
                original_queue: Arc::new(Mutex::new(Vec::new())),
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
                            let _ = daemon_for_mpris
                                .mpv_tx
                                .send(MpvCommand::SetPause(false))
                                .await;
                        }
                        drop(state);
                        daemon_for_mpris.broadcast_and_save().await;
                    }
                    DaemonCommand::Pause => {
                        let mut state = daemon_for_mpris.state.lock().await;
                        if state.status == PlaybackStatus::Playing {
                            state.status = PlaybackStatus::Paused;
                            let _ = daemon_for_mpris
                                .mpv_tx
                                .send(MpvCommand::SetPause(true))
                                .await;
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
            let initial_state = self.state.lock().await.clone();
            mpris.update(&initial_state);
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

    async fn handle_mpv_event(self: &Arc<Self>, event: MpvEvent) {
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
                    if state.status == PlaybackStatus::Buffering {
                        if !is_paused {
                            state.status = PlaybackStatus::Playing;
                        }
                    } else {
                        state.status = if is_paused {
                            PlaybackStatus::Paused
                        } else {
                            PlaybackStatus::Playing
                        };
                    }
                }
            }
            MpvEvent::FileLoaded => {
                if state.current_track.is_some() {
                    state.status = PlaybackStatus::Playing;
                    let _ = self.mpv_tx.send(MpvCommand::SetPause(false)).await;
                }
            }
            MpvEvent::FileError => {
                let cur = state.current_track.clone();
                drop(state);
                if let Some(track) = cur {
                    warn!(
                        "MPV reported file error loading {}. Invalidating cache and retrying.",
                        track.video_id
                    );
                    {
                        let db = self.db.lock().await;
                        let _ = db.remove_stream_url(&track.video_id);
                    }
                    self.play_track(track).await;
                }
                return;
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

    pub async fn play_track(self: &Arc<Self>, track: Track) {
        let my_gen = {
            let mut gen = self.play_generation.lock().await;
            *gen = gen.wrapping_add(1);
            *gen
        };

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

        let daemon = Arc::clone(self);
        let video_id = track.video_id.clone();

        tokio::spawn(async move {
            let db = Arc::clone(&daemon.db);
            match resolve_stream_url(&video_id, &db).await {
                Ok(url) => {
                    let current_gen = *daemon.play_generation.lock().await;
                    if current_gen != my_gen {
                        debug!("Stream URL resolved for {} but discarded due to generation mismatch ({} vs {})", video_id, current_gen, my_gen);
                        return;
                    }

                    let _ = daemon.mpv_tx.send(MpvCommand::LoadFile(url)).await;
                    let mut state = daemon.state.lock().await;
                    state.status = PlaybackStatus::Playing;
                    drop(state);
                    daemon.broadcast_and_save().await;
                }
                Err(e) => {
                    let current_gen = *daemon.play_generation.lock().await;
                    if current_gen != my_gen {
                        return;
                    }
                    error!("Failed to resolve stream URL for {}: {}", video_id, e);
                    let mut state = daemon.state.lock().await;
                    state.status = PlaybackStatus::Stopped;
                    drop(state);
                    daemon.broadcast_and_save().await;
                }
            }
        });
    }

    pub async fn queue_tracks(self: &Arc<Self>, tracks: Vec<Track>, start_idx: usize) {
        if tracks.is_empty() {
            return;
        }

        let selected_track;
        {
            let mut state = self.state.lock().await;
            let mut orig_queue = self.original_queue.lock().await;

            if state.shuffle_enabled {
                *orig_queue = tracks.clone();
                let mut shuffled = tracks;
                let valid_start = start_idx.min(shuffled.len().saturating_sub(1));
                shuffle_tracks_preserving_current(&mut shuffled, valid_start);
                selected_track = shuffled.first().cloned();
                state.queue = shuffled;
                state.queue_index = 0;
            } else {
                orig_queue.clear();
                selected_track = tracks.get(start_idx).cloned();
                state.queue = tracks;
                state.queue_index = start_idx;
            }
        }

        if let Some(track) = selected_track {
            self.play_track(track).await;
        }
    }

    pub async fn enqueue_track(self: &Arc<Self>, track: Track) {
        let should_play = {
            let mut state = self.state.lock().await;
            let mut orig_queue = self.original_queue.lock().await;
            if state.shuffle_enabled {
                orig_queue.push(track.clone());
            }
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

    pub async fn next_track(self: &Arc<Self>) {
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

    pub async fn prev_track(self: &Arc<Self>) {
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

    pub async fn toggle(self: &Arc<Self>) {
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

    pub async fn toggle_shuffle(&self) {
        let mut state = self.state.lock().await;
        let new_shuffle = !state.shuffle_enabled;
        state.shuffle_enabled = new_shuffle;

        let mut orig_queue = self.original_queue.lock().await;

        if new_shuffle {
            // Turning ON shuffle
            if !state.queue.is_empty() {
                *orig_queue = state.queue.clone();
                let cur_idx = state.queue_index.min(state.queue.len().saturating_sub(1));
                shuffle_tracks_preserving_current(&mut state.queue, cur_idx);
                state.queue_index = 0;
            }
        } else {
            // Turning OFF shuffle
            if !orig_queue.is_empty() {
                let cur_vid = state.current_track.as_ref().map(|t| t.video_id.as_str());
                let restored_idx = if let Some(vid) = cur_vid {
                    orig_queue
                        .iter()
                        .position(|t| t.video_id == vid)
                        .unwrap_or(0)
                } else {
                    0
                };
                state.queue = orig_queue.clone();
                state.queue_index = restored_idx;
                orig_queue.clear();
            }
        }

        *self.prefetch_triggered.lock().await = false;
        drop(orig_queue);
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

        // Sync with YouTube Music account if authenticated
        let auth = crate::innertube::auth::AuthConfig::load(&self.config.auth_file);
        if auth.is_authenticated() {
            let vid = video_id.clone();
            tokio::spawn(async move {
                let client = crate::innertube::client::InnertubeClient::new(auth);
                let _ = client.like_track(&vid, is_liked).await;
            });
        }

        // Send desktop notification
        let i18n = current_i18n(&self.config);
        let msg = if is_liked {
            i18n.tf(
                "notification.liked",
                [
                    ("title", current_track.title),
                    ("artist", current_track.artist),
                ],
            )
        } else {
            i18n.tf(
                "notification.unliked",
                [
                    ("title", current_track.title),
                    ("artist", current_track.artist),
                ],
            )
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
        let is_stopped = state.status == PlaybackStatus::Stopped;

        let status_json = serde_json::json!({
            "text": if is_playing {
                format!("▶ {} - {}", artist, title)
            } else if is_paused {
                format!("⏸ {} - {}", artist, title)
            } else if is_tui_open {
                i18n.t("status.open")
            } else {
                "🎵 YTM-RS".to_string()
            },
            "tooltip": if is_playing {
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
                    ],
                )
            } else if is_paused {
                i18n.tf(
                    "status.paused_tooltip",
                    [
                        ("title", title.clone()),
                        ("artist", artist.clone()),
                        ("volume", state.volume.to_string()),
                    ],
                )
            } else if is_tui_open {
                i18n.t("status.window_open_tooltip")
            } else {
                i18n.t("status.background_tooltip")
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
                DaemonCommand::ToggleShuffle => {
                    self.toggle_shuffle().await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shuffle_tracks_preserving_current() {
        let make_track = |id: &str| Track::new(id, id, "Artist", "Album", 180, "");

        // Empty vector
        let mut tracks: Vec<Track> = Vec::new();
        shuffle_tracks_preserving_current(&mut tracks, 0);
        assert!(tracks.is_empty());

        // Single track
        let mut tracks = vec![make_track("t1")];
        shuffle_tracks_preserving_current(&mut tracks, 0);
        assert_eq!(tracks[0].video_id, "t1");

        // Two tracks
        let mut tracks = vec![make_track("t1"), make_track("t2")];
        shuffle_tracks_preserving_current(&mut tracks, 1);
        assert_eq!(tracks[0].video_id, "t2");
        assert_eq!(tracks[1].video_id, "t1");

        // Multiple tracks: ensure selected track is ALWAYS at index 0 and all tracks preserved
        let mut tracks = vec![
            make_track("t0"),
            make_track("t1"),
            make_track("t2"),
            make_track("t3"),
            make_track("t4"),
        ];
        shuffle_tracks_preserving_current(&mut tracks, 3);
        assert_eq!(tracks[0].video_id, "t3");
        assert_eq!(tracks.len(), 5);

        let mut ids: Vec<String> = tracks.iter().map(|t| t.video_id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["t0", "t1", "t2", "t3", "t4"]);
    }
}
