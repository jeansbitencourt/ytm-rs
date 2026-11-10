use anyhow::Result;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::audio::{DaemonCommand, PlaybackStatus, PlayerState};

pub struct MprisServer {
    controls: Option<MediaControls>,
}

impl MprisServer {
    pub fn new(cmd_tx: mpsc::Sender<DaemonCommand>) -> Result<Self> {
        let hwnd = None;
        let config = PlatformConfig {
            dbus_name: "ytmrs",
            display_name: "YTM-RS",
            hwnd,
        };

        match MediaControls::new(config) {
            Ok(mut controls) => {
                let attach_res = controls.attach(move |event: MediaControlEvent| {
                    let cmd = match event {
                        MediaControlEvent::Play => Some(DaemonCommand::Play),
                        MediaControlEvent::Pause => Some(DaemonCommand::Pause),
                        MediaControlEvent::Toggle => Some(DaemonCommand::Toggle),
                        MediaControlEvent::Next => Some(DaemonCommand::Next),
                        MediaControlEvent::Previous => Some(DaemonCommand::Prev),
                        MediaControlEvent::Stop => Some(DaemonCommand::Stop),
                        MediaControlEvent::SetPosition(_pos) => {
                            // Convert to seek if desired
                            None
                        }
                        _ => None,
                    };

                    if let Some(c) = cmd {
                        let _ = cmd_tx.try_send(c);
                    }
                });

                if let Err(e) = attach_res {
                    error!("Failed to attach MPRIS2 event handler: {:?}", e);
                    Ok(Self { controls: None })
                } else {
                    info!("MPRIS2 D-Bus server initialized (org.mpris.MediaPlayer2.ytmrs)");
                    Ok(Self {
                        controls: Some(controls),
                    })
                }
            }
            Err(e) => {
                error!("Could not initialize MPRIS2 controls: {:?}", e);
                Ok(Self { controls: None })
            }
        }
    }

    pub fn update(&mut self, state: &PlayerState) {
        let controls = match &mut self.controls {
            Some(c) => c,
            None => return,
        };

        let pos_dur =
            Duration::try_from_secs_f64(state.position_secs.max(0.0)).unwrap_or(Duration::ZERO);
        let pos = MediaPosition(pos_dur);

        let playback = match state.status {
            PlaybackStatus::Playing => MediaPlayback::Playing {
                progress: Some(pos),
            },
            PlaybackStatus::Paused => MediaPlayback::Paused {
                progress: Some(pos),
            },
            PlaybackStatus::Stopped => MediaPlayback::Stopped,
            PlaybackStatus::Buffering => MediaPlayback::Paused { progress: None },
        };

        let _ = controls.set_playback(playback);

        if let Some(track) = &state.current_track {
            let metadata = MediaMetadata {
                title: Some(&track.title),
                artist: Some(&track.artist),
                album: Some(&track.album),
                duration: Some(Duration::from_secs(track.duration_secs as u64)),
                cover_url: if track.thumbnail_url.is_empty() {
                    None
                } else {
                    Some(&track.thumbnail_url)
                },
            };
            let _ = controls.set_metadata(metadata);
        } else {
            let metadata = MediaMetadata {
                title: Some("YTM-RS"),
                artist: Some("YouTube Music"),
                album: None,
                duration: None,
                cover_url: None,
            };
            let _ = controls.set_metadata(metadata);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mpris_event_from_non_tokio_thread() {
        let (cmd_tx, mut rx) = mpsc::channel(32);

        // Simulate souvlaki calling the closure in an external OS thread with NO Tokio runtime
        let thread_handle = std::thread::spawn(move || {
            let res = cmd_tx.try_send(DaemonCommand::Toggle);
            assert!(res.is_ok(), "try_send must succeed from a non-tokio thread");
        });

        thread_handle.join().expect("thread join failed");
        let received = rx.recv().await;
        assert_eq!(received, Some(DaemonCommand::Toggle));
    }

    #[test]
    fn test_duration_safe_conversion() {
        let neg_pos: f64 = -5.0;
        let pos_dur = Duration::try_from_secs_f64(neg_pos.max(0.0)).unwrap_or(Duration::ZERO);
        assert_eq!(pos_dur, Duration::ZERO);

        let valid_pos: f64 = 12.34;
        let pos_dur_valid =
            Duration::try_from_secs_f64(valid_pos.max(0.0)).unwrap_or(Duration::ZERO);
        assert_eq!(pos_dur_valid.as_secs(), 12);
    }
}
