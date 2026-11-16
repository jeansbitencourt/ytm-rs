mod commands;
mod events;
mod ipc;
mod playback;
mod recovery;
mod status;

pub use ipc::{ensure_daemon_running, send_daemon_command};

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

use super::mpv_ipc::{MpvCommand, MpvEvent, MpvManager};
use super::{DaemonCommand, PlayerState};
use crate::cache::Database;
use crate::config::AppConfig;
use crate::i18n::I18n;
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
