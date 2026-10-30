use std::time::Duration;
use anyhow::Result;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
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
                let tx = cmd_tx.clone();
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
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let _ = tx.send(c).await;
                        });
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

        let playback = match state.status {
            PlaybackStatus::Playing => {
                let pos = MediaPosition(Duration::from_secs_f64(state.position_secs));
                MediaPlayback::Playing { progress: Some(pos) }
            }
            PlaybackStatus::Paused => {
                let pos = MediaPosition(Duration::from_secs_f64(state.position_secs));
                MediaPlayback::Paused { progress: Some(pos) }
            }
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
