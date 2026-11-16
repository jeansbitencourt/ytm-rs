use std::sync::Arc;
use tracing::warn;

use super::super::mpv_ipc::MpvCommand;
use super::super::PlaybackStatus;
use super::{current_i18n, shuffle_tracks_preserving_current, AudioDaemon};
use crate::innertube::Track;

impl AudioDaemon {
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

    pub(super) async fn next_track_if_generation(
        self: &Arc<Self>,
        expected_generation: Option<u64>,
    ) {
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

    pub(super) async fn stop_if_generation(&self, expected_generation: Option<u64>) {
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
}
