use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, error, warn};

use super::super::mpv_ipc::MpvCommand;
use super::super::prefetch::{resolve_music_video_stream_url, resolve_stream_url, ResolvedMedia};
use super::super::PlaybackStatus;
use super::{
    current_i18n, wait_for_generation_change, AudioDaemon, PendingVariant, MPV_START_TIMEOUT,
};
use crate::innertube::auth::AuthConfig;
use crate::innertube::client::InnertubeClient;
use crate::innertube::Track;

impl AudioDaemon {
    pub async fn play_track(self: &Arc<Self>, track: Track) {
        self.begin_track(track, None, true, true).await;
    }

    pub(super) async fn begin_track(
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

    pub(super) async fn switch_variant(
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
            let result: Result<Option<(ResolvedMedia, String)>> = tokio::select! {
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

    pub(super) async fn resolve_music_video_id(
        &self,
        audio_video_id: &str,
    ) -> Result<Option<String>> {
        let mut client = InnertubeClient::new(AuthConfig::load(&self.config.auth_file));
        client
            .ensure_valid_token(Some(&self.config.auth_file))
            .await?;
        client.music_video_counterpart_id(audio_video_id).await
    }

    pub(super) async fn update_track_video_id(&self, generation: u64, track: &Track) {
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

    pub(super) async fn fallback_to_audio(
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
}
