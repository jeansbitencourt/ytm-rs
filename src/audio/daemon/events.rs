use std::sync::{atomic::Ordering, Arc};
use tracing::debug;

use super::super::mpv_ipc::{MpvCommand, MpvEvent};
use super::super::prefetch::spawn_prefetch;
use super::super::PlaybackStatus;
use super::{current_i18n, AudioDaemon};

impl AudioDaemon {
    pub(super) async fn handle_mpv_event(self: &Arc<Self>, event: MpvEvent) {
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
}
