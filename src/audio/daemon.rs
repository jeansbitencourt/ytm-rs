use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

use super::mpv_ipc::{MpvCommand, MpvEvent, MpvManager};
use super::prefetch::{resolve_music_video_stream_url, resolve_stream_url, spawn_prefetch};
use super::{DaemonCommand, DaemonResponse, PlaybackStatus, PlayerState};
use crate::cache::Database;
use crate::config::AppConfig;
use crate::i18n::I18n;
use crate::innertube::auth::AuthConfig;
use crate::innertube::client::InnertubeClient;
use crate::innertube::Track;

const MPV_START_TIMEOUT: Duration = Duration::from_secs(30);
const MPV_RETRY_DELAY: Duration = Duration::from_secs(5);

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

#[derive(Debug, Clone)]
struct PendingVariant {
    generation: u64,
    is_video: bool,
    position_secs: f64,
    paused: bool,
}

pub struct AudioDaemon {
    config: AppConfig,
    state: Arc<Mutex<PlayerState>>,
    db: Arc<Mutex<Database>>,
    bcast_tx: broadcast::Sender<PlayerState>,
    mpv_manager: MpvManager,
    mpv_controller: Mutex<super::mpv_ipc::MpvController>,
    mpv_event_tx: mpsc::Sender<MpvEvent>,
    pending_variant: Mutex<Option<PendingVariant>>,
    prefetch_triggered: Arc<Mutex<bool>>,
    prefetch_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    tui_clients: Arc<Mutex<usize>>,
    play_generation: Arc<Mutex<u64>>,
    retry_count: Arc<Mutex<u8>>,
    original_queue: Arc<Mutex<Vec<Track>>>,
    shutdown_tx: broadcast::Sender<()>,
    daemon_running: AtomicBool,
    reconnecting_mpv: AtomicBool,
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
            .start(mpv_event_tx.clone(), Arc::clone(&play_generation))
            .await?;

        let state = Arc::new(Mutex::new(PlayerState::default()));
        let (shutdown_tx, _) = broadcast::channel(1);

        Ok((
            Self {
                config,
                state,
                db,
                bcast_tx,
                mpv_manager,
                mpv_controller: Mutex::new(mpv_controller),
                mpv_event_tx,
                pending_variant: Mutex::new(None),
                prefetch_triggered: Arc::new(Mutex::new(false)),
                prefetch_task: Mutex::new(None),
                tui_clients: Arc::new(Mutex::new(0)),
                play_generation,
                retry_count: Arc::new(Mutex::new(0)),
                original_queue: Arc::new(Mutex::new(Vec::new())),
                shutdown_tx,
                daemon_running: AtomicBool::new(true),
                reconnecting_mpv: AtomicBool::new(false),
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
        let _ = self.send_mpv_command(MpvCommand::Stop { generation }).await;

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
        if self
            .send_mpv_command(MpvCommand::Quit(quit_tx))
            .await
            .is_ok()
        {
            let _ = tokio::time::timeout(Duration::from_secs(2), quit_rx).await;
        }
        self.write_status_json().await;
        drop(listener);
        let _ = std::fs::remove_file(sock_path);
        info!("Audio daemon stopped");
        Ok(())
    }

    async fn send_mpv_command(&self, command: MpvCommand) -> Result<()> {
        let mut controller = self.mpv_controller.lock().await;
        match controller.send(command).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let command = error.0;
                if matches!(&command, MpvCommand::Quit(_)) {
                    anyhow::bail!("MPV worker channel closed during daemon shutdown");
                }

                warn!("MPV command channel closed; reconnecting before retrying the command");
                let replacement = self
                    .mpv_manager
                    .start(self.mpv_event_tx.clone(), Arc::clone(&self.play_generation))
                    .await
                    .context("Could not restart the MPV command channel")?;
                *controller = replacement;
                controller
                    .send(command)
                    .await
                    .map_err(|_| anyhow::anyhow!("MPV worker channel closed after reconnect"))
            }
        }
    }

    async fn handle_mpv_event(self: &Arc<Self>, event: MpvEvent) {
        if !self.daemon_running.load(Ordering::Acquire) {
            return;
        }

        if let MpvEvent::ConnectionLost(message) = &event {
            self.recover_after_mpv_disconnect(message.clone()).await;
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

        if matches!(&event, MpvEvent::PlaybackRestart { .. }) {
            let pending = {
                let mut pending = self.pending_variant.lock().await;
                if pending
                    .as_ref()
                    .is_some_and(|pending| pending.generation == event_generation)
                {
                    pending.take()
                } else {
                    None
                }
            };
            if let Some(pending) = pending {
                drop(generation);
                if pending.position_secs > 0.0 {
                    let _ = self
                        .send_mpv_command(MpvCommand::SeekAbsolute(pending.position_secs))
                        .await;
                }
                let _ = self
                    .send_mpv_command(MpvCommand::SetPause(pending.paused))
                    .await;
                if !self.is_current_generation(event_generation).await {
                    return;
                }
                {
                    let mut state = self.state.lock().await;
                    state.video_mode = pending.is_video;
                    state.position_secs = pending.position_secs;
                    state.status = if pending.paused {
                        PlaybackStatus::Paused
                    } else {
                        PlaybackStatus::Playing
                    };
                    state.error_message = None;
                    if pending.is_video {
                        state.video_error_message = None;
                    }
                }
                self.reconnecting_mpv.store(false, Ordering::Release);
                self.broadcast_and_save().await;
                return;
            }
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
                                let should_prefetch = self
                                    .config
                                    .load_playback_auth()
                                    .map(|settings| {
                                        !settings.source.is_enabled() || settings.prefetch_enabled
                                    })
                                    .unwrap_or(false);
                                if should_prefetch {
                                    let mut prefetch_task = self.prefetch_task.lock().await;
                                    if let Some(previous_task) = prefetch_task.take() {
                                        previous_task.abort();
                                    }
                                    *prefetch_task = Some(spawn_prefetch(
                                        next_track.video_id,
                                        Arc::clone(&self.db),
                                        self.config.clone(),
                                    ));
                                }
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
                let was_video_mode = state.video_mode;
                let position_secs = state.position_secs;
                let paused = state.status == PlaybackStatus::Paused;
                let pending = {
                    let mut pending = self.pending_variant.lock().await;
                    if pending
                        .as_ref()
                        .is_some_and(|pending| pending.generation == event_generation)
                    {
                        pending.take()
                    } else {
                        None
                    }
                };
                drop(state);
                if let Some(track) = cur {
                    drop(generation);
                    if pending.as_ref().is_some_and(|pending| pending.is_video) {
                        let pending = pending.expect("checked pending video transition");
                        self.switch_variant(
                            track.clone(),
                            Some(track.video_id.clone()),
                            false,
                            pending.position_secs,
                            pending.paused,
                            Some(current_i18n(&self.config).t("player.video_failed")),
                        )
                        .await;
                        return;
                    }
                    if pending.is_some() {
                        self.retry_after_mpv_error(event_generation, track, message)
                            .await;
                        return;
                    }
                    if was_video_mode {
                        self.switch_variant(
                            track.clone(),
                            Some(track.video_id.clone()),
                            false,
                            position_secs,
                            paused,
                            Some(current_i18n(&self.config).t("player.video_failed")),
                        )
                        .await;
                        return;
                    }
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
            state.video_mode = false;
            state.video_error_message = None;

            let db = self.db.lock().await;
            state.is_liked = db.is_track_liked(&track.video_id).unwrap_or(false);
            if record_history {
                let _ = db.add_history(&track);
            }
            my_generation
        };

        *self.pending_variant.lock().await = None;

        *self.prefetch_triggered.lock().await = false;
        if let Err(e) = self
            .send_mpv_command(MpvCommand::Stop {
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
                _ = wait_for_generation_change(Arc::clone(&generation_watch), my_generation) => return,
                result = resolve_stream_url(&video_id, &db, &daemon.config) => result,
            };

            match result {
                Ok(resolved) => {
                    if !daemon.is_current_generation(my_generation).await {
                        return;
                    }

                    // Try a newly resolved URL immediately on the first attempt.
                    // If MPV rejects it, the retry path waits for the URL to age.
                    if !reset_retry_count && !resolved.wait_before_use.is_zero() {
                        debug!(
                            "Waiting {} seconds for the stream URL for {} to become usable",
                            resolved.wait_before_use.as_secs_f32(),
                            video_id
                        );
                        tokio::select! {
                            biased;
                            _ = wait_for_generation_change(Arc::clone(&generation_watch), my_generation) => return,
                            _ = tokio::time::sleep(resolved.wait_before_use) => {}
                        }
                    }

                    if !daemon.is_current_generation(my_generation).await {
                        return;
                    }

                    let source_still_current = daemon
                        .config
                        .load_playback_auth()
                        .map(|settings| settings.source == resolved.auth_source)
                        .unwrap_or(false);
                    if !source_still_current {
                        daemon
                            .fail_playback_if_current(
                                my_generation,
                                "Playback authentication changed during stream resolution; start the track again.".to_string(),
                            )
                            .await;
                        return;
                    }
                    if let Err(e) = daemon
                        .send_mpv_command(MpvCommand::LoadFile {
                            media: resolved,
                            generation: my_generation,
                            title: format!("{} — {}", track.title, track.artist),
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

    pub async fn set_video_mode(self: &Arc<Self>, enabled: bool) {
        let pending = self.pending_variant.lock().await.clone();
        let (track, position_secs, paused, current_mode) = {
            let state = self.state.lock().await;
            (
                state.current_track.clone(),
                pending
                    .as_ref()
                    .map(|pending| pending.position_secs)
                    .unwrap_or(state.position_secs),
                pending
                    .as_ref()
                    .map(|pending| pending.paused)
                    .unwrap_or(state.status == PlaybackStatus::Paused),
                pending
                    .as_ref()
                    .map(|pending| pending.is_video)
                    .unwrap_or(state.video_mode),
            )
        };

        let Some(track) = track else {
            let mut state = self.state.lock().await;
            state.video_error_message = Some(current_i18n(&self.config).t("player.video_no_track"));
            drop(state);
            self.broadcast_and_save().await;
            return;
        };

        if enabled == current_mode {
            if !enabled && pending.is_none() {
                let mut state = self.state.lock().await;
                state.video_error_message = None;
                drop(state);
                self.broadcast_and_save().await;
            }
            return;
        }

        let media_id = if enabled {
            track.music_video_id.clone()
        } else {
            Some(track.video_id.clone())
        };

        self.switch_variant(track, media_id, enabled, position_secs, paused, None)
            .await;
    }

    pub async fn toggle_video_mode(self: &Arc<Self>) {
        let pending_mode = self
            .pending_variant
            .lock()
            .await
            .as_ref()
            .map(|pending| pending.is_video);
        let current_mode = pending_mode.unwrap_or(self.state.lock().await.video_mode);
        self.set_video_mode(!current_mode).await;
    }

    async fn switch_variant(
        self: &Arc<Self>,
        track: Track,
        media_id: Option<String>,
        is_video: bool,
        position_secs: f64,
        paused: bool,
        video_error_message: Option<String>,
    ) {
        let generation = {
            let mut generation = self.play_generation.lock().await;
            *generation = generation.wrapping_add(1);
            let generation = *generation;
            let mut state = self.state.lock().await;
            state.status = PlaybackStatus::Buffering;
            state.error_message = None;
            state.video_error_message = video_error_message;
            state.position_secs = position_secs.max(0.0);
            generation
        };

        *self.pending_variant.lock().await = Some(PendingVariant {
            generation,
            is_video,
            position_secs: position_secs.max(0.0),
            paused,
        });
        if let Err(error) = self.send_mpv_command(MpvCommand::Stop { generation }).await {
            self.fail_playback_if_current(
                generation,
                format!("Could not stop MPV before switching media: {error}"),
            )
            .await;
            return;
        }
        self.broadcast_and_save().await;

        let daemon = Arc::clone(self);
        let generation_watch = Arc::clone(&self.play_generation);
        tokio::spawn(async move {
            let mut track = track;
            let result: Result<Option<(super::prefetch::ResolvedMedia, String)>> = tokio::select! {
                biased;
                _ = wait_for_generation_change(Arc::clone(&generation_watch), generation) => return,
                result = async {
                    if is_video {
                        let video_id = match media_id.as_deref() {
                            Some(video_id) => Some(video_id.to_string()),
                            None => daemon.resolve_music_video_id(&track.video_id).await?,
                        };
                        let Some(video_id) = video_id else {
                            return Ok(None);
                        };
                        let media = resolve_music_video_stream_url(&video_id, &daemon.config).await?;
                        Ok(Some((media, video_id)))
                    } else {
                        let audio_id = media_id
                            .as_deref()
                            .unwrap_or(&track.video_id)
                            .to_string();
                        let db = Arc::clone(&daemon.db);
                        let media = resolve_stream_url(&audio_id, &db, &daemon.config).await?;
                        Ok(Some((media, audio_id)))
                    }
                } => result,
            };

            let (resolved, resolved_media_id) = match result {
                Ok(Some(resolved)) => resolved,
                Ok(None) => {
                    daemon
                        .fallback_to_audio(
                            generation,
                            track,
                            position_secs,
                            paused,
                            current_i18n(&daemon.config).t("player.video_unavailable"),
                        )
                        .await;
                    return;
                }
                Err(error) if is_video => {
                    warn!(
                        "Could not resolve video variant for {}: {:#}",
                        track.video_id, error
                    );
                    daemon
                        .fallback_to_audio(
                            generation,
                            track,
                            position_secs,
                            paused,
                            current_i18n(&daemon.config).t("player.video_failed"),
                        )
                        .await;
                    return;
                }
                Err(error) => {
                    daemon
                        .fail_playback_if_current(
                            generation,
                            format!("Could not resolve the audio stream: {error:#}"),
                        )
                        .await;
                    return;
                }
            };

            if !daemon.is_current_generation(generation).await {
                return;
            }
            if is_video {
                track.music_video_id = Some(resolved_media_id);
                daemon.update_track_video_id(generation, &track).await;
                if !daemon.is_current_generation(generation).await {
                    return;
                }
            }
            let source_still_current = daemon
                .config
                .load_playback_auth()
                .map(|settings| settings.source == resolved.auth_source)
                .unwrap_or(false);
            if !source_still_current {
                daemon
                    .fail_playback_if_current(
                        generation,
                        "Playback authentication changed during stream resolution; start the track again."
                            .to_string(),
                    )
                    .await;
                return;
            }

            if let Err(error) = daemon
                .send_mpv_command(MpvCommand::LoadFile {
                    media: resolved,
                    generation,
                    title: format!("{} — {}", track.title, track.artist),
                })
                .await
            {
                daemon
                    .fail_playback_if_current(
                        generation,
                        format!("Could not send the media to MPV: {error}"),
                    )
                    .await;
                return;
            }

            let watchdog_daemon = Arc::clone(&daemon);
            tokio::spawn(async move {
                tokio::time::sleep(MPV_START_TIMEOUT).await;
                watchdog_daemon
                    .fail_buffering_if_current(
                        generation,
                        format!(
                            "MPV did not start playback within {} seconds",
                            MPV_START_TIMEOUT.as_secs()
                        ),
                    )
                    .await;
            });
        });
    }

    async fn resolve_music_video_id(&self, audio_video_id: &str) -> Result<Option<String>> {
        let mut client = InnertubeClient::new(AuthConfig::load(&self.config.auth_file));
        client
            .ensure_valid_token(Some(&self.config.auth_file))
            .await?;
        client.music_video_counterpart_id(audio_video_id).await
    }

    async fn update_track_video_id(&self, generation: u64, track: &Track) {
        if !self.is_current_generation(generation).await {
            return;
        }

        let state = {
            let mut state = self.state.lock().await;
            if state
                .current_track
                .as_ref()
                .is_some_and(|current| current.video_id == track.video_id)
            {
                state.current_track = Some(track.clone());
            }
            for queued_track in &mut state.queue {
                if queued_track.video_id == track.video_id {
                    queued_track.music_video_id = track.music_video_id.clone();
                }
            }
            state.clone()
        };

        {
            let mut original_queue = self.original_queue.lock().await;
            for queued_track in original_queue.iter_mut() {
                if queued_track.video_id == track.video_id {
                    queued_track.music_video_id = track.music_video_id.clone();
                }
            }
        }

        if let Some(music_video_id) = track.music_video_id.as_deref() {
            let db = self.db.lock().await;
            if let Err(error) = db.update_music_video_id(&track.video_id, music_video_id) {
                warn!(
                    "Could not cache the music video counterpart for {}: {}",
                    track.video_id, error
                );
            }
        }

        let _ = self.bcast_tx.send(state);
        self.write_status_json().await;
    }

    async fn fallback_to_audio(
        self: &Arc<Self>,
        failed_generation: u64,
        track: Track,
        position_secs: f64,
        paused: bool,
        video_error_message: String,
    ) {
        let generation = {
            let mut generation = self.play_generation.lock().await;
            if *generation != failed_generation {
                return;
            }
            *generation = generation.wrapping_add(1);
            let generation = *generation;
            let mut state = self.state.lock().await;
            state.status = PlaybackStatus::Buffering;
            state.video_mode = false;
            state.video_error_message = Some(video_error_message);
            state.position_secs = position_secs.max(0.0);
            generation
        };
        *self.pending_variant.lock().await = Some(PendingVariant {
            generation,
            is_video: false,
            position_secs: position_secs.max(0.0),
            paused,
        });
        if let Err(error) = self.send_mpv_command(MpvCommand::Stop { generation }).await {
            self.fail_playback_if_current(
                generation,
                format!("Could not restore audio after video playback failed: {error}"),
            )
            .await;
            return;
        }
        self.broadcast_and_save().await;

        let daemon = Arc::clone(self);
        let generation_watch = Arc::clone(&self.play_generation);
        tokio::spawn(async move {
            let db = Arc::clone(&daemon.db);
            let result = tokio::select! {
                biased;
                _ = wait_for_generation_change(Arc::clone(&generation_watch), generation) => return,
                result = resolve_stream_url(&track.video_id, &db, &daemon.config) => result,
            };
            let resolved = match result {
                Ok(resolved) => resolved,
                Err(error) => {
                    daemon
                        .fail_playback_if_current(
                            generation,
                            format!("Could not restore the audio stream: {error:#}"),
                        )
                        .await;
                    return;
                }
            };
            if !daemon.is_current_generation(generation).await {
                return;
            }
            let source_still_current = daemon
                .config
                .load_playback_auth()
                .map(|settings| settings.source == resolved.auth_source)
                .unwrap_or(false);
            if !source_still_current {
                daemon
                    .fail_playback_if_current(
                        generation,
                        "Playback authentication changed during stream resolution; start the track again."
                            .to_string(),
                    )
                    .await;
                return;
            }
            if let Err(error) = daemon
                .send_mpv_command(MpvCommand::LoadFile {
                    media: resolved,
                    generation,
                    title: format!("{} — {}", track.title, track.artist),
                })
                .await
            {
                daemon
                    .fail_playback_if_current(
                        generation,
                        format!("Could not restore audio in MPV: {error}"),
                    )
                    .await;
                return;
            }
            let watchdog_daemon = Arc::clone(&daemon);
            tokio::spawn(async move {
                tokio::time::sleep(MPV_START_TIMEOUT).await;
                watchdog_daemon
                    .fail_buffering_if_current(
                        generation,
                        format!(
                            "MPV did not resume audio within {} seconds",
                            MPV_START_TIMEOUT.as_secs()
                        ),
                    )
                    .await;
            });
        });
    }

    async fn recover_after_mpv_disconnect(self: &Arc<Self>, message: String) {
        if self.reconnecting_mpv.swap(true, Ordering::AcqRel) {
            return;
        }
        error!("Lost connection to MPV: {}", message);

        let (track, position_secs, paused, volume) = {
            let mut generation = self.play_generation.lock().await;
            *generation = generation.wrapping_add(1);
            let mut state = self.state.lock().await;
            let track = state.current_track.clone();
            let position_secs = state.position_secs;
            let paused = state.status == PlaybackStatus::Paused;
            let volume = state.volume;
            state.video_mode = false;
            state.status = if track.is_some() {
                PlaybackStatus::Buffering
            } else {
                PlaybackStatus::Stopped
            };
            state.error_message = None;
            (track, position_secs, paused, volume)
        };
        self.broadcast_and_save().await;

        let controller = match self
            .mpv_manager
            .start(self.mpv_event_tx.clone(), Arc::clone(&self.play_generation))
            .await
        {
            Ok(controller) => controller,
            Err(error) => {
                self.reconnecting_mpv.store(false, Ordering::Release);
                let generation = *self.play_generation.lock().await;
                self.fail_playback_if_current(
                    generation,
                    format!("Could not restart MPV after its playback window closed: {error}"),
                )
                .await;
                return;
            }
        };
        *self.mpv_controller.lock().await = controller;
        let _ = self.send_mpv_command(MpvCommand::SetVolume(volume)).await;

        if let Some(track) = track {
            self.switch_variant(
                track.clone(),
                Some(track.video_id.clone()),
                false,
                position_secs,
                paused,
                None,
            )
            .await;
        } else {
            self.reconnecting_mpv.store(false, Ordering::Release);
        }
    }

    async fn retry_after_mpv_error(
        self: &Arc<Self>,
        failed_generation: u64,
        track: Track,
        message: String,
    ) {
        let auth_configured = self
            .config
            .load_playback_auth()
            .map(|settings| settings.source.is_enabled())
            .unwrap_or(true);
        if auth_configured {
            self.fail_playback_if_current(
                failed_generation,
                "Configured playback credentials could not be used. Automatic retries are disabled; check the selected source or disable playback authentication.".to_string(),
            )
            .await;
            return;
        }

        enum RetryAction {
            Delayed(u64),
            RefreshUrl,
            Exhausted,
        }

        let retry_action = {
            let mut generation = self.play_generation.lock().await;
            if *generation != failed_generation {
                return;
            }
            let mut retry_count = self.retry_count.lock().await;
            match *retry_count {
                0 => {
                    *retry_count = 1;
                    *generation = generation.wrapping_add(1);
                    RetryAction::Delayed(*generation)
                }
                1 => {
                    *retry_count = 2;
                    RetryAction::RefreshUrl
                }
                _ => RetryAction::Exhausted,
            }
        };

        match retry_action {
            RetryAction::Delayed(expected_generation) => {
                warn!(
                    "MPV failed to load {} ({}); retrying the same stream after {} seconds",
                    track.video_id,
                    message,
                    MPV_RETRY_DELAY.as_secs()
                );
                let daemon = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(MPV_RETRY_DELAY).await;
                    daemon
                        .begin_track(track, Some(expected_generation), false, false)
                        .await;
                });
            }
            RetryAction::RefreshUrl => {
                warn!(
                    "MPV failed to load {} ({}); invalidating its cached stream and refreshing it",
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
            RetryAction::Exhausted => {
                self.fail_playback_if_current(
                    failed_generation,
                    format!("MPV could not play this track: {message}"),
                )
                .await;
            }
        }
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
        state.video_mode = false;
        drop(state);
        drop(current_generation);
        *self.pending_variant.lock().await = None;
        self.reconnecting_mpv.store(false, Ordering::Release);
        if let Err(e) = self
            .send_mpv_command(MpvCommand::Stop {
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
            if let Err(e) = self.send_mpv_command(MpvCommand::SetPause(false)).await {
                warn!("Could not send play command to MPV: {}", e);
            }
        }
    }

    pub async fn pause(&self) {
        let is_playing = self.state.lock().await.status == PlaybackStatus::Playing;
        if is_playing {
            if let Err(e) = self.send_mpv_command(MpvCommand::SetPause(true)).await {
                warn!("Could not send pause command to MPV: {}", e);
            }
        }
    }

    pub async fn stop(self: &Arc<Self>) {
        self.stop_if_generation(None).await;
    }

    pub async fn playback_auth_changed(self: &Arc<Self>) {
        // Stop any in-flight resolver or media request so old credentials and
        // per-file MPV headers are discarded immediately after configuration changes.
        if let Some(prefetch_task) = self.prefetch_task.lock().await.take() {
            prefetch_task.abort();
        }
        self.stop().await;
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
            state.video_mode = false;
            state.video_error_message = None;
            next_generation
        };

        *self.pending_variant.lock().await = None;
        if let Err(e) = self.send_mpv_command(MpvCommand::Stop { generation }).await {
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
        let _ = self.send_mpv_command(MpvCommand::SetVolume(clamped)).await;
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
