use std::sync::{atomic::Ordering, Arc};
use tracing::{error, warn};

use super::super::mpv_ipc::MpvCommand;
use super::super::PlaybackStatus;
use super::{AudioDaemon, MPV_RETRY_DELAY};
use crate::innertube::Track;

impl AudioDaemon {
    pub(super) async fn recover_after_mpv_disconnect(self: &Arc<Self>, message: String) {
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

    pub(super) async fn retry_after_mpv_error(
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

    pub(super) async fn is_current_generation(&self, expected: u64) -> bool {
        *self.play_generation.lock().await == expected
    }

    pub(super) async fn fail_playback_if_current(&self, generation: u64, message: String) {
        self.finish_failed_playback(generation, message, false)
            .await;
    }

    pub(super) async fn fail_buffering_if_current(&self, generation: u64, message: String) {
        self.finish_failed_playback(generation, message, true).await;
    }

    pub(super) async fn finish_failed_playback(
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
}
