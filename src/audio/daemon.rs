use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

use super::mpv_ipc::{MpvCommand, MpvEvent, MpvManager};
use super::prefetch::{resolve_stream_url, spawn_prefetch};
use super::{DaemonCommand, DaemonResponse, PlaybackStatus, PlayerState};
use crate::cache::Database;
use crate::config::AppConfig;
use crate::i18n::I18n;
use crate::innertube::Track;

const MPV_START_TIMEOUT: Duration = Duration::from_secs(30);

fn current_i18n(config: &AppConfig) -> I18n {
    let preference = AppConfig::load()
        .map(|settings| settings.settings.general.language)
        .unwrap_or(config.settings.general.language);
    I18n::new(preference)
}

async fn wait_for_generation_change(generation: Arc<Mutex<u64>>, expected: u64) {
    loop {
        if *generation.lock().await != expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
    retry_count: Arc<Mutex<u8>>,
    original_queue: Arc<Mutex<Vec<Track>>>,
    shutdown_tx: broadcast::Sender<()>,
    daemon_running: AtomicBool,
    status_write_lock: Mutex<()>,
}

impl AudioDaemon {
    pub async fn new(config: AppConfig) -> Result<(Self, mpsc::Receiver<MpvEvent>)> {
        let db = Database::open(&config.db_file)?;
        let db = Arc::new(Mutex::new(db));

        let (bcast_tx, _) = broadcast::channel(128);
        let (mpv_event_tx, mpv_event_rx) = mpsc::channel(64);
        let play_generation = Arc::new(Mutex::new(0));

        let mpv_manager = MpvManager::new(config.mpv_sock.clone());
        let mpv_controller = mpv_manager
            .start(mpv_event_tx, Arc::clone(&play_generation))
            .await?;

        // Internal channel from daemon methods to mpv controller
        let (mpv_cmd_tx, mut mpv_cmd_rx) = mpsc::channel::<MpvCommand>(32);
        tokio::spawn(async move {
            while let Some(cmd) = mpv_cmd_rx.recv().await {
                if let Err(e) = mpv_controller.send(cmd).await {
                    error!("MPV command worker stopped: {}", e);
                    break;
                }
            }
        });

        let state = Arc::new(Mutex::new(PlayerState::default()));
        let (shutdown_tx, _) = broadcast::channel(1);

        Ok((
            Self {
                config,
                state,
                db,
                bcast_tx,
                mpv_tx: mpv_cmd_tx,
                prefetch_triggered: Arc::new(Mutex::new(false)),
                tui_clients: Arc::new(Mutex::new(0)),
                play_generation,
                retry_count: Arc::new(Mutex::new(0)),
                original_queue: Arc::new(Mutex::new(Vec::new())),
                shutdown_tx,
                daemon_running: AtomicBool::new(true),
                status_write_lock: Mutex::new(()),
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
        self.daemon_running.store(true, Ordering::Release);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let generation = *self.play_generation.lock().await;
        let _ = self.mpv_tx.send(MpvCommand::Stop { generation }).await;

        // Write initial status.json
        self.write_status_json().await;

        // MPRIS2 Integration
        let (mpris_cmd_tx, mut mpris_cmd_rx) = mpsc::channel(32);
        let daemon_for_mpris = Arc::clone(&self);
        tokio::spawn(async move {
            while let Some(cmd) = mpris_cmd_rx.recv().await {
                match cmd {
                    DaemonCommand::Toggle => daemon_for_mpris.toggle().await,
                    DaemonCommand::Play => daemon_for_mpris.play().await,
                    DaemonCommand::Pause => daemon_for_mpris.pause().await,
                    DaemonCommand::Next => daemon_for_mpris.next_track().await,
                    DaemonCommand::Prev => daemon_for_mpris.prev_track().await,
                    DaemonCommand::Stop => daemon_for_mpris.stop().await,
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
        let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                _ = heartbeat.tick() => self.write_status_json().await,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let daemon = Arc::clone(&self);
                        tokio::spawn(async move {
                            if let Err(e) = daemon.handle_client(stream).await {
                                debug!("Client connection ended: {}", e);
                            }
                        });
                    }
                    Err(e) => error!("Error accepting client connection: {}", e),
                }
            }
        }

        self.stop().await;
        self.daemon_running.store(false, Ordering::Release);
        *self.tui_clients.lock().await = 0;
        let (quit_tx, quit_rx) = oneshot::channel();
        if self.mpv_tx.send(MpvCommand::Quit(quit_tx)).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(2), quit_rx).await;
        }
        self.write_status_json().await;
        drop(listener);
        let _ = std::fs::remove_file(sock_path);
        info!("Audio daemon stopped");
        Ok(())
    }

    async fn handle_mpv_event(self: &Arc<Self>, event: MpvEvent) {
        if !self.daemon_running.load(Ordering::Acquire) {
            return;
        }

        if let MpvEvent::ConnectionLost(message) = &event {
            error!("Lost connection to MPV: {}", message);
            let mut generation = self.play_generation.lock().await;
            *generation = generation.wrapping_add(1);
            let mut state = self.state.lock().await;
            state.status = PlaybackStatus::Stopped;
            state.error_message = Some(format!("MPV connection lost: {message}"));
            drop(state);
            drop(generation);
            self.broadcast_and_save().await;
            return;
        }

        let event_generation = match &event {
            MpvEvent::TimePos { generation, .. }
            | MpvEvent::Duration { generation, .. }
            | MpvEvent::Pause { generation, .. }
            | MpvEvent::PlaybackRestart { generation }
            | MpvEvent::Stopped { generation }
            | MpvEvent::FileError { generation, .. }
            | MpvEvent::Eof { generation } => *generation,
            MpvEvent::ConnectionLost(_) => return,
        };
        let generation = self.play_generation.lock().await;
        if *generation != event_generation {
            debug!(
                "Discarding stale MPV event for generation {} (current {})",
                event_generation, *generation
            );
            return;
        }

        let mut state = self.state.lock().await;
        match event {
            MpvEvent::TimePos { position, .. } => {
                state.position_secs = position.max(0.0);
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
            MpvEvent::Duration { duration, .. } => {
                if duration.is_finite() && duration >= 0.0 {
                    state.duration_secs = duration;
                }
            }
            MpvEvent::Pause { paused, .. } => {
                if state.current_track.is_some()
                    && matches!(
                        state.status,
                        PlaybackStatus::Playing | PlaybackStatus::Paused
                    )
                {
                    state.status = if paused {
                        PlaybackStatus::Paused
                    } else {
                        PlaybackStatus::Playing
                    };
                }
            }
            MpvEvent::PlaybackRestart { .. } => {
                if state.current_track.is_some() {
                    state.status = PlaybackStatus::Playing;
                    state.error_message = None;
                }
            }
            MpvEvent::Stopped { .. } => {
                if matches!(
                    state.status,
                    PlaybackStatus::Playing | PlaybackStatus::Paused
                ) {
                    state.status = PlaybackStatus::Stopped;
                    state.position_secs = 0.0;
                    state.error_message = None;
                }
            }
            MpvEvent::FileError { message, .. } => {
                let cur = state.current_track.clone();
                drop(state);
                if let Some(track) = cur {
                    drop(generation);
                    self.retry_after_mpv_error(event_generation, track, message)
                        .await;
                    return;
                }
                return;
            }
            MpvEvent::Eof { .. } => {
                drop(state);
                drop(generation);
                self.next_track_if_generation(Some(event_generation)).await;
                return;
            }
            MpvEvent::ConnectionLost(_) => unreachable!(),
        }

        let state_clone = state.clone();
        drop(state);
        drop(generation);
        let _ = self.bcast_tx.send(state_clone);
        self.write_status_json().await;
    }

    pub async fn play_track(self: &Arc<Self>, track: Track) {
        self.begin_track(track, None, true, true).await;
    }

    async fn begin_track(
        self: &Arc<Self>,
        track: Track,
        expected_generation: Option<u64>,
        reset_retry_count: bool,
        record_history: bool,
    ) {
        let my_generation = {
            let mut generation = self.play_generation.lock().await;
            if expected_generation.is_some_and(|expected| expected != *generation) {
                return;
            }
            *generation = generation.wrapping_add(1);
            let my_generation = *generation;
            if reset_retry_count {
                *self.retry_count.lock().await = 0;
            }

            let mut state = self.state.lock().await;
            state.status = PlaybackStatus::Buffering;
            state.error_message = None;
            state.position_secs = 0.0;
            state.duration_secs = track.duration_secs as f64;
            state.current_track = Some(track.clone());

            let db = self.db.lock().await;
            state.is_liked = db.is_track_liked(&track.video_id).unwrap_or(false);
            if record_history {
                let _ = db.add_history(&track);
            }
            my_generation
        };

        *self.prefetch_triggered.lock().await = false;
        if let Err(e) = self
            .mpv_tx
            .send(MpvCommand::Stop {
                generation: my_generation,
            })
            .await
        {
            self.fail_playback_if_current(
                my_generation,
                format!("Could not stop the previous MPV stream: {e}"),
            )
            .await;
            return;
        }
        self.broadcast_and_save().await;

        let daemon = Arc::clone(self);
        let video_id = track.video_id.clone();
        let generation_watch = Arc::clone(&self.play_generation);

        tokio::spawn(async move {
            let db = Arc::clone(&daemon.db);
            let result = tokio::select! {
                biased;
                _ = wait_for_generation_change(generation_watch, my_generation) => return,
                result = resolve_stream_url(&video_id, &db) => result,
            };

            match result {
                Ok(url) => {
                    if !daemon.is_current_generation(my_generation).await {
                        return;
                    }
                    if let Err(e) = daemon
                        .mpv_tx
                        .send(MpvCommand::LoadFile {
                            url,
                            generation: my_generation,
                        })
                        .await
                    {
                        daemon
                            .fail_playback_if_current(
                                my_generation,
                                format!("Could not send the stream to MPV: {e}"),
                            )
                            .await;
                    } else {
                        debug!(
                            "Stream URL resolved for {}; waiting for MPV playback confirmation",
                            video_id
                        );
                        let watchdog_daemon = Arc::clone(&daemon);
                        tokio::spawn(async move {
                            tokio::time::sleep(MPV_START_TIMEOUT).await;
                            watchdog_daemon
                                .fail_buffering_if_current(
                                    my_generation,
                                    format!(
                                        "MPV did not start playback within {} seconds",
                                        MPV_START_TIMEOUT.as_secs()
                                    ),
                                )
                                .await;
                        });
                    }
                }
                Err(e) => {
                    error!("Failed to resolve stream URL for {}: {:#}", video_id, e);
                    daemon
                        .fail_playback_if_current(
                            my_generation,
                            format!("Could not resolve the audio stream: {e:#}"),
                        )
                        .await;
                }
            }
        });
    }

    async fn retry_after_mpv_error(
        self: &Arc<Self>,
        failed_generation: u64,
        track: Track,
        message: String,
    ) {
        let should_retry = {
            let generation = self.play_generation.lock().await;
            if *generation != failed_generation {
                return;
            }
            let mut retry_count = self.retry_count.lock().await;
            if *retry_count == 0 {
                *retry_count = 1;
                true
            } else {
                false
            }
        };

        if !should_retry {
            self.fail_playback_if_current(
                failed_generation,
                format!("MPV could not play this track: {message}"),
            )
            .await;
            return;
        }

        warn!(
            "MPV failed to load {} ({}); invalidating its cached stream and retrying once",
            track.video_id, message
        );
        {
            let db = self.db.lock().await;
            if let Err(e) = db.remove_stream_url(&track.video_id) {
                warn!("Could not invalidate cached stream URL: {}", e);
            }
        }
        self.begin_track(track, Some(failed_generation), false, false)
            .await;
    }

    async fn is_current_generation(&self, expected: u64) -> bool {
        *self.play_generation.lock().await == expected
    }

    async fn fail_playback_if_current(&self, generation: u64, message: String) {
        self.finish_failed_playback(generation, message, false)
            .await;
    }

    async fn fail_buffering_if_current(&self, generation: u64, message: String) {
        self.finish_failed_playback(generation, message, true).await;
    }

    async fn finish_failed_playback(
        &self,
        generation: u64,
        message: String,
        only_if_buffering: bool,
    ) {
        let mut current_generation = self.play_generation.lock().await;
        if *current_generation != generation {
            return;
        }
        let mut state = self.state.lock().await;
        if only_if_buffering && state.status != PlaybackStatus::Buffering {
            return;
        }
        *current_generation = current_generation.wrapping_add(1);
        let stop_generation = *current_generation;
        state.status = PlaybackStatus::Stopped;
        state.error_message = Some(message.chars().take(500).collect());
        state.position_secs = 0.0;
        drop(state);
        drop(current_generation);
        if let Err(e) = self
            .mpv_tx
            .send(MpvCommand::Stop {
                generation: stop_generation,
            })
            .await
        {
            warn!("Could not stop MPV after playback failure: {}", e);
        }
        self.broadcast_and_save().await;
    }

    pub async fn queue_tracks(self: &Arc<Self>, tracks: Vec<Track>, start_idx: usize) {
        if tracks.is_empty() {
            return;
        }

        let selected_track;
        let start_idx = start_idx.min(tracks.len() - 1);
        {
            let mut state = self.state.lock().await;
            let mut orig_queue = self.original_queue.lock().await;

            if state.shuffle_enabled {
                *orig_queue = tracks.clone();
                let mut shuffled = tracks;
                shuffle_tracks_preserving_current(&mut shuffled, start_idx);
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
        self.next_track_if_generation(None).await;
    }

    async fn next_track_if_generation(self: &Arc<Self>, expected_generation: Option<u64>) {
        let current_generation = self.play_generation.lock().await;
        if expected_generation.is_some_and(|expected| expected != *current_generation) {
            return;
        }
        let generation = *current_generation;
        let next_track = {
            let mut state = self.state.lock().await;
            if state.queue.is_empty() {
                None
            } else if state.queue_index + 1 < state.queue.len() {
                state.queue_index += 1;
                Some(state.queue[state.queue_index].clone())
            } else {
                None
            }
        };
        drop(current_generation);

        if let Some(track) = next_track {
            self.begin_track(track, Some(generation), true, true).await;
        } else {
            self.stop_if_generation(Some(generation)).await;
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

    pub async fn play(&self) {
        let is_paused = self.state.lock().await.status == PlaybackStatus::Paused;
        if is_paused {
            if let Err(e) = self.mpv_tx.send(MpvCommand::SetPause(false)).await {
                warn!("Could not send play command to MPV: {}", e);
            }
        }
    }

    pub async fn pause(&self) {
        let is_playing = self.state.lock().await.status == PlaybackStatus::Playing;
        if is_playing {
            if let Err(e) = self.mpv_tx.send(MpvCommand::SetPause(true)).await {
                warn!("Could not send pause command to MPV: {}", e);
            }
        }
    }

    pub async fn stop(self: &Arc<Self>) {
        self.stop_if_generation(None).await;
    }

    async fn stop_if_generation(&self, expected_generation: Option<u64>) {
        let generation = {
            let mut generation = self.play_generation.lock().await;
            if expected_generation.is_some_and(|expected| expected != *generation) {
                return;
            }
            *generation = generation.wrapping_add(1);
            let next_generation = *generation;
            let mut state = self.state.lock().await;
            state.status = PlaybackStatus::Stopped;
            state.error_message = None;
            state.position_secs = 0.0;
            next_generation
        };

        if let Err(e) = self.mpv_tx.send(MpvCommand::Stop { generation }).await {
            warn!("Could not send stop command to MPV: {}", e);
        }
        self.broadcast_and_save().await;
    }

    pub async fn toggle(self: &Arc<Self>) {
        let (status, queued_track) = {
            let state = self.state.lock().await;
            (state.status, state.queue.get(state.queue_index).cloned())
        };
        match status {
            PlaybackStatus::Playing => {
                self.pause().await;
            }
            PlaybackStatus::Paused => {
                self.play().await;
            }
            PlaybackStatus::Stopped => {
                if let Some(track) = queued_track {
                    self.play_track(track).await;
                }
            }
            PlaybackStatus::Buffering => {}
        }
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
