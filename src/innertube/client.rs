use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT};
use serde_json::{json, Value};
use tracing::warn;

use super::auth::AuthConfig;
use super::Track;

const INNERTUBE_BASE_URL: &str = "https://music.youtube.com/youtubei/v1";
const WEB_REMIX_ORIGIN: &str = "https://music.youtube.com";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

#[derive(Clone)]
pub struct InnertubeClient {
    http: reqwest::Client,
    pub auth: AuthConfig,
}

impl InnertubeClient {
    pub fn new(auth: AuthConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .build()
            .unwrap_or_default();
        Self { http, auth }
    }

    fn default_context() -> Value {
        json!({
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": "1.20260101.01.00",
                "hl": "pt-BR",
                "gl": "BR"
            }
        })
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ORIGIN, HeaderValue::from_static(WEB_REMIX_ORIGIN));
        headers.insert(REFERER, HeaderValue::from_static("https://music.youtube.com/"));
        headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));

        if let Some(cookies) = &self.auth.cookies {
            if let Ok(val) = HeaderValue::from_str(cookies) {
                headers.insert(COOKIE, val);
            }
        }

        if let Some(auth_hdr) = self.auth.get_sapisid_auth_header(WEB_REMIX_ORIGIN) {
            if let Ok(val) = HeaderValue::from_str(&auth_hdr) {
                headers.insert(AUTHORIZATION, val);
            }
        } else if let Some(token) = &self.auth.oauth_token {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", token)) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        headers
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let url = format!("{}/search", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "query": query,
            // Filter to Songs / Tracks when possible
            "params": "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D"
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send search request to Innertube")?;

        let body: Value = resp.json().await.context("Failed to parse search response JSON")?;
        let tracks = parse_search_results(&body);
        if tracks.is_empty() {
            // Fallback to search without params
            let payload_fallback = json!({
                "context": Self::default_context(),
                "query": query,
            });
            let headers = self.build_headers();
            if let Ok(r) = self.http.post(&url).headers(headers).json(&payload_fallback).send().await {
                if let Ok(b) = r.json::<Value>().await {
                    return Ok(parse_search_results(&b));
                }
            }
        }
        Ok(tracks)
    }

    pub async fn radio(&self, video_id: &str) -> Result<Vec<Track>> {
        let url = format!("{}/next", INNERTUBE_BASE_URL);
        let playlist_id = format!("RDAMVM{}", video_id);
        let payload = json!({
            "context": Self::default_context(),
            "videoId": video_id,
            "playlistId": playlist_id,
            "isAudioOnly": true
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send radio request to Innertube")?;

        let body: Value = resp.json().await.context("Failed to parse radio response JSON")?;
        Ok(parse_radio_results(&body))
    }

    pub async fn get_liked_music(&self) -> Result<Vec<Track>> {
        let url = format!("{}/browse", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "browseId": "FEmusic_liked_videos"
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send browse request to Innertube")?;

        let body: Value = resp.json().await.context("Failed to parse browse response JSON")?;
        Ok(parse_playlist_results(&body))
    }

    pub async fn like_track(&self, video_id: &str, like: bool) -> Result<()> {
        let endpoint = if like { "like/like" } else { "like/removelike" };
        let url = format!("{}/{}", INNERTUBE_BASE_URL, endpoint);
        let payload = json!({
            "context": Self::default_context(),
            "target": {
                "videoId": video_id
            }
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send like request to Innertube")?;

        if !resp.status().is_success() {
            warn!("Innertube like endpoint returned status: {}", resp.status());
        }
        Ok(())
    }
}

pub fn parse_duration_to_secs(text: &str) -> u32 {
    let parts: Vec<&str> = text.split(':').collect();
    match parts.len() {
        1 => parts[0].parse().unwrap_or(0),
        2 => {
            let m: u32 = parts[0].parse().unwrap_or(0);
            let s: u32 = parts[1].parse().unwrap_or(0);
            m * 60 + s
        }
        3 => {
            let h: u32 = parts[0].parse().unwrap_or(0);
            let m: u32 = parts[1].parse().unwrap_or(0);
            let s: u32 = parts[2].parse().unwrap_or(0);
            h * 3600 + m * 60 + s
        }
        _ => 0,
    }
}

pub fn parse_search_results(val: &Value) -> Vec<Track> {
    let mut tracks = Vec::new();
    // Search can return contents in tabs or itemSectionRenderers
    let items = val
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .or_else(|| val.pointer("/contents/sectionListRenderer/contents"));

    if let Some(Value::Array(sections)) = items {
        for sec in sections {
            let contents = sec
                .pointer("/musicShelfRenderer/contents")
                .or_else(|| sec.pointer("/musicCardShelfRenderer/contents"))
                .or_else(|| sec.pointer("/itemSectionRenderer/contents"));

            if let Some(Value::Array(items)) = contents {
                for item in items {
                    if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(track) = parse_music_responsive_item(renderer) {
                            tracks.push(track);
                        }
                    }
                }
            }
        }
    }
    tracks
}

pub fn parse_music_responsive_item(renderer: &Value) -> Option<Track> {
    // 1. VideoId
    let video_id = renderer
        .pointer("/playlistItemData/videoId")
        .or_else(|| renderer.pointer("/doubleTapCommand/watchEndpoint/videoId"))
        .or_else(|| renderer.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    // Title is in flexColumns[0]
    let title_runs = flex_columns[0].pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?.as_array()?;
    let title = title_runs.iter().map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or("")).collect::<Vec<_>>().join("");

    // Artist / Album / Duration in flexColumns[1]
    let mut artist = "Artista Desconhecido".to_string();
    let mut album = "".to_string();
    let mut duration_secs = 0;

    if flex_columns.len() > 1 {
        if let Some(subtitle_runs) = flex_columns[1].pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs").and_then(|r| r.as_array()) {
            let texts: Vec<&str> = subtitle_runs.iter().filter_map(|r| r.get("text").and_then(|t| t.as_str())).filter(|t| *t != " • ").collect();
            if !texts.is_empty() {
                artist = texts[0].to_string();
            }
            if texts.len() > 1 {
                album = texts[1].to_string();
            }
            if let Some(last) = texts.last() {
                duration_secs = parse_duration_to_secs(last);
            }
        }
    }

    // Fixed column duration fallback
    if duration_secs == 0 {
        if let Some(dur_text) = renderer.pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text").and_then(|t| t.as_str()) {
            duration_secs = parse_duration_to_secs(dur_text);
        }
    }

    // Thumbnail
    let thumbnail_url = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(Track::new(video_id, title, artist, album, duration_secs, thumbnail_url))
}

pub fn parse_radio_results(val: &Value) -> Vec<Track> {
    let mut tracks = Vec::new();
    let items = val.pointer("/continuationContents/playlistPanelContinuation/contents")
        .or_else(|| val.pointer("/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs/0/tabRenderer/content/musicQueueRenderer/content/playlistPanelRenderer/contents"))
        .or_else(|| val.pointer("/currentVideoEndpoint/watchEndpoint/playlistPanelRenderer/contents"));

    if let Some(Value::Array(list)) = items {
        for item in list {
            if let Some(renderer) = item.get("playlistPanelVideoRenderer") {
                if let Some(track) = parse_playlist_panel_video(renderer) {
                    tracks.push(track);
                }
            }
        }
    }
    tracks
}

pub fn parse_playlist_panel_video(renderer: &Value) -> Option<Track> {
    let video_id = renderer.get("videoId")?.as_str()?.to_string();
    let title = renderer.pointer("/title/runs/0/text")?.as_str()?.to_string();

    let mut artist = "Artista Desconhecido".to_string();
    let mut album = "".to_string();
    let mut duration_secs = 0;

    if let Some(runs) = renderer.pointer("/longBylineText/runs").or_else(|| renderer.pointer("/shortBylineText/runs")).and_then(|r| r.as_array()) {
        let texts: Vec<&str> = runs.iter().filter_map(|r| r.get("text").and_then(|t| t.as_str())).filter(|t| *t != " • ").collect();
        if !texts.is_empty() {
            artist = texts[0].to_string();
        }
        if texts.len() > 1 {
            album = texts[1].to_string();
        }
    }

    if let Some(dur_text) = renderer.pointer("/lengthText/runs/0/text").and_then(|t| t.as_str()) {
        duration_secs = parse_duration_to_secs(dur_text);
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(Track::new(video_id, title, artist, album, duration_secs, thumbnail_url))
}

pub fn parse_playlist_results(val: &Value) -> Vec<Track> {
    let mut tracks = Vec::new();
    let items = val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicPlaylistShelfRenderer/contents");
    if let Some(Value::Array(list)) = items {
        for item in list {
            if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                if let Some(track) = parse_music_responsive_item(renderer) {
                    tracks.push(track);
                }
            }
        }
    }
    tracks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration_to_secs("04:18"), 258);
        assert_eq!(parse_duration_to_secs("01:02:15"), 3735);
        assert_eq!(parse_duration_to_secs("45"), 45);
    }

    #[test]
    fn test_parse_music_responsive_item() {
        let raw = json!({
            "playlistItemData": {
                "videoId": "abc12345"
            },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Nightcall" }
                            ]
                        }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Kavinsky" },
                                { "text": " • " },
                                { "text": "OutRun" },
                                { "text": " • " },
                                { "text": "04:18" }
                            ]
                        }
                    }
                }
            ],
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/test.jpg" }
                        ]
                    }
                }
            }
        });

        let track = parse_music_responsive_item(&raw).unwrap();
        assert_eq!(track.video_id, "abc12345");
        assert_eq!(track.title, "Nightcall");
        assert_eq!(track.artist, "Kavinsky");
        assert_eq!(track.album, "OutRun");
        assert_eq!(track.duration_secs, 258);
        assert_eq!(track.duration_text, "04:18");
    }
}
