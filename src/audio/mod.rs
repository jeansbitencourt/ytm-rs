use crate::innertube::Track;
use serde::{Deserialize, Serialize};

pub mod daemon;
pub mod mpv_ipc;
pub mod prefetch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
    Buffering,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        PlaybackStatus::Stopped
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    pub current_track: Option<Track>,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub volume: u8,
    pub is_liked: bool,
    pub queue: Vec<Track>,
    pub queue_index: usize,
    pub is_radio_active: bool,
    #[serde(default)]
    pub shuffle_enabled: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            current_track: None,
            position_secs: 0.0,
            duration_secs: 0.0,
            volume: 75,
            is_liked: false,
            queue: Vec::new(),
            queue_index: 0,
            is_radio_active: false,
            shuffle_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DaemonCommand {
    PlayTrack(Track),
    QueueTracks(Vec<Track>, usize),
    EnqueueTrack(Track),
    Toggle,
    Play,
    Pause,
    Stop,
    Next,
    Prev,
    Seek(i64),
    VolumeDelta(i32),
    SetVolume(u8),
    ToggleLike,
    ToggleShuffle,
    StartRadio(Track),
    GetState,
    Subscribe,
    StopDaemon,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DaemonResponse {
    Ok,
    State(Box<PlayerState>),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_command_serialization() {
        let cmd = DaemonCommand::Seek(15);
        let serialized = serde_json::to_string(&cmd).unwrap();
        let deserialized: DaemonCommand = serde_json::from_str(&serialized).unwrap();
        assert_eq!(cmd, deserialized);

        let shuf_cmd = DaemonCommand::ToggleShuffle;
        let shuf_ser = serde_json::to_string(&shuf_cmd).unwrap();
        let shuf_deser: DaemonCommand = serde_json::from_str(&shuf_ser).unwrap();
        assert_eq!(shuf_cmd, shuf_deser);

        let mut state = PlayerState::default();
        assert!(!state.shuffle_enabled);
        state.shuffle_enabled = true;
        let resp = DaemonResponse::State(Box::new(state));
        let serialized_resp = serde_json::to_string(&resp).unwrap();
        let deserialized_resp: DaemonResponse = serde_json::from_str(&serialized_resp).unwrap();
        assert_eq!(resp, deserialized_resp);
    }
}
