use serde::{Deserialize, Serialize};

pub mod auth;
pub mod client;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u32,
    pub duration_text: String,
    pub thumbnail_url: String,
    pub is_liked: bool,
}

impl Track {
    pub fn new(
        video_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
        duration_secs: u32,
        thumbnail_url: impl Into<String>,
    ) -> Self {
        let duration_text = format_duration(duration_secs);
        Self {
            video_id: video_id.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_secs,
            duration_text,
            thumbnail_url: thumbnail_url.into(),
            is_liked: false,
        }
    }
}

pub fn format_duration(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub count_text: String,
    pub thumbnail_url: String,
}
