use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT};
use serde_json::{json, Value};
use tracing::warn;

use super::auth::AuthConfig;
use super::{AlbumSummary, ArtistSummary, SearchFilter, Track};

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

    pub async fn search_albums(&self, query: &str) -> Result<Vec<AlbumSummary>> {
        let url = format!("{}/search", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "query": query,
            "params": SearchFilter::Albums.params()
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send album search request to Innertube")?;

        let body: Value = resp.json().await.context("Failed to parse album search response JSON")?;
        Ok(parse_album_search_results(&body))
    }

    pub async fn search_artists(&self, query: &str) -> Result<Vec<ArtistSummary>> {
        let url = format!("{}/search", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "query": query,
            "params": SearchFilter::Artists.params()
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send artist search request to Innertube")?;

        let body: Value = resp.json().await.context("Failed to parse artist search response JSON")?;
        Ok(parse_artist_search_results(&body))
    }

    pub async fn get_album_tracks(&self, browse_id: &str) -> Result<(String, Vec<Track>)> {
        let url = format!("{}/browse", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "browseId": browse_id
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send album browse request to Innertube")?;

        let body: Value = resp.json().await.context("Failed to parse album browse response JSON")?;
        Ok(parse_album_browse_tracks(&body))
    }

    pub async fn get_artist_tracks(&self, browse_id: &str) -> Result<(String, Vec<Track>)> {
        let url = format!("{}/browse", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "browseId": browse_id
        });

        let headers = self.build_headers();
        let resp = self.http.post(&url)
            .headers(headers.clone())
            .json(&payload)
            .send()
            .await
            .context("Failed to send artist browse request to Innertube")?;

        let body: Value = resp.json().await.context("Failed to parse artist browse response JSON")?;

        let artist_name = body
            .pointer("/header/musicImmersiveHeaderRenderer/title/runs/0/text")
            .or_else(|| body.pointer("/header/musicVisualHeaderRenderer/title/runs/0/text"))
            .and_then(|t| t.as_str())
            .unwrap_or("Artista")
            .to_string();

        // Check if there is a full songs playlist shelf (e.g. browseId starts with "VL")
        let mut songs_browse_id = None;
        if let Some(sections) = body.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents").and_then(|v| v.as_array()) {
            for sec in sections {
                if let Some(bid) = sec.pointer("/musicShelfRenderer/bottomEndpoint/browseEndpoint/browseId").and_then(|v| v.as_str()) {
                    if bid.starts_with("VL") {
                        songs_browse_id = Some(bid.to_string());
                        break;
                    }
                }
            }
        }

        if let Some(bid) = songs_browse_id {
            let payload2 = json!({
                "context": Self::default_context(),
                "browseId": bid
            });
            if let Ok(r2) = self.http.post(&url).headers(headers).json(&payload2).send().await {
                if let Ok(b2) = r2.json::<Value>().await {
                    let mut playlist_tracks = parse_playlist_results(&b2);
                    if !playlist_tracks.is_empty() {
                        for t in &mut playlist_tracks {
                            if t.artist == "Artista Desconhecido" {
                                t.artist = artist_name.clone();
                            }
                        }
                        return Ok((artist_name, playlist_tracks));
                    }
                }
            }
        }

        // Fallback: extract tracks from the initial artist shelves
        let mut fallback_tracks = Vec::new();
        if let Some(sections) = body.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents").and_then(|v| v.as_array()) {
            for sec in sections {
                if let Some(contents) = sec.pointer("/musicShelfRenderer/contents").and_then(|v| v.as_array()) {
                    for item in contents {
                        if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
                            if let Some(mut track) = parse_music_responsive_item(renderer) {
                                if track.artist == "Artista Desconhecido" {
                                    track.artist = artist_name.clone();
                                }
                                fallback_tracks.push(track);
                            }
                        }
                    }
                }
            }
        }

        Ok((artist_name, fallback_tracks))
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
        .or_else(|| renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    // Title is in flexColumns[0]
    let title_runs = flex_columns[0].pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?.as_array()?;
    let title = title_runs.iter().map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or("")).collect::<Vec<_>>().join("");

    let mut artist = "Artista Desconhecido".to_string();
    let mut album = "".to_string();
    let mut duration_secs = 0;

    if flex_columns.len() == 2 {
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
    } else if flex_columns.len() > 2 {
        for col in &flex_columns[1..] {
            if let Some(runs) = col.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs").and_then(|r| r.as_array()) {
                for r in runs {
                    let text = r.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
                    if text == "•" || text.is_empty() {
                        continue;
                    }
                    let page_type = r.pointer("/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
                        .and_then(|p| p.as_str());

                    if page_type == Some("MUSIC_PAGE_TYPE_ARTIST") && (artist == "Artista Desconhecido" || artist.is_empty()) {
                        artist = text.to_string();
                    } else if page_type == Some("MUSIC_PAGE_TYPE_ALBUM") {
                        album = text.to_string();
                    } else if artist == "Artista Desconhecido" && !text.starts_with("Tocou ") && !text.chars().any(|c| c.is_ascii_digit()) {
                        artist = text.to_string();
                    }
                }
            }
        }
    }

    // Fixed column duration fallback
    if duration_secs == 0 {
        if let Some(fixed_cols) = renderer.get("fixedColumns").and_then(|f| f.as_array()) {
            for col in fixed_cols {
                if let Some(dur_text) = col.pointer("/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text").and_then(|t| t.as_str()) {
                    let d = parse_duration_to_secs(dur_text);
                    if d > 0 {
                        duration_secs = d;
                        break;
                    }
                }
            }
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

pub fn parse_album_search_results(val: &Value) -> Vec<AlbumSummary> {
    let mut albums = Vec::new();
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
                        if let Some(album) = parse_album_item(renderer) {
                            albums.push(album);
                        }
                    }
                }
            }
        }
    }
    albums
}

pub fn parse_album_item(renderer: &Value) -> Option<AlbumSummary> {
    let browse_id = renderer
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .or_else(|| renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let playlist_id = renderer
        .pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchPlaylistEndpoint/playlistId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    let title_runs = flex_columns[0].pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?.as_array()?;
    let title = title_runs.iter().map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or("")).collect::<Vec<_>>().join("");

    let mut artist = "Artista Desconhecido".to_string();
    let mut year = "".to_string();

    if flex_columns.len() > 1 {
        if let Some(subtitle_runs) = flex_columns[1].pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs").and_then(|r| r.as_array()) {
            let texts: Vec<&str> = subtitle_runs.iter().filter_map(|r| r.get("text").and_then(|t| t.as_str())).filter(|t| *t != " • ").collect();
            for t in &texts {
                let s = t.trim();
                if s.chars().all(|c| c.is_ascii_digit()) && s.len() == 4 {
                    year = s.to_string();
                } else if s != "Álbum" && s != "Album" && s != "Single" && s != "EP" && (artist == "Artista Desconhecido" || artist.is_empty()) {
                    artist = s.to_string();
                }
            }
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(AlbumSummary {
        browse_id,
        playlist_id,
        title,
        artist,
        year,
        thumbnail_url,
    })
}

pub fn parse_artist_search_results(val: &Value) -> Vec<ArtistSummary> {
    let mut artists = Vec::new();
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
                        if let Some(artist) = parse_artist_item(renderer) {
                            artists.push(artist);
                        }
                    }
                }
            }
        }
    }
    artists
}

pub fn parse_artist_item(renderer: &Value) -> Option<ArtistSummary> {
    let browse_id = renderer
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .or_else(|| renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|v| v.as_str())?
        .to_string();

    let flex_columns = renderer.get("flexColumns")?.as_array()?;
    if flex_columns.is_empty() {
        return None;
    }

    let name_runs = flex_columns[0].pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?.as_array()?;
    let name = name_runs.iter().map(|r| r.get("text").and_then(|t| t.as_str()).unwrap_or("")).collect::<Vec<_>>().join("");

    let mut subscribers_text = "".to_string();
    if flex_columns.len() > 1 {
        if let Some(subtitle_runs) = flex_columns[1].pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs").and_then(|r| r.as_array()) {
            let texts: Vec<&str> = subtitle_runs.iter().filter_map(|r| r.get("text").and_then(|t| t.as_str())).filter(|t| *t != " • ").collect();
            if texts.len() > 1 {
                subscribers_text = texts[1].to_string();
            } else if !texts.is_empty() {
                subscribers_text = texts[0].to_string();
            }
        }
    }

    let thumbnail_url = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    Some(ArtistSummary {
        browse_id,
        name,
        subscribers_text,
        thumbnail_url,
    })
}

pub fn parse_album_browse_tracks(val: &Value) -> (String, Vec<Track>) {
    let header = val.pointer("/header/musicResponsiveHeaderRenderer")
        .or_else(|| val.pointer("/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicResponsiveHeaderRenderer"));

    let album_title = header
        .and_then(|h| h.pointer("/title/runs/0/text"))
        .and_then(|t| t.as_str())
        .unwrap_or("Álbum")
        .to_string();

    let album_artist = header
        .and_then(|h| h.pointer("/straplineTextOne/runs/0/text").or_else(|| h.pointer("/subtitle/runs/0/text")))
        .and_then(|t| t.as_str())
        .unwrap_or("Artista Desconhecido")
        .to_string();

    let header_thumb = header
        .and_then(|h| h.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"))
        .and_then(|th| th.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();

    let mut tracks = Vec::new();

    let mut raw_items = Vec::new();
    if let Some(items) = val.pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents/0/musicShelfRenderer/contents").and_then(|v| v.as_array()) {
        raw_items.extend(items.clone());
    } else if let Some(items) = val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicShelfRenderer/contents").and_then(|v| v.as_array()) {
        raw_items.extend(items.clone());
    } else if let Some(sections) = val.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents").and_then(|v| v.as_array()) {
        for s in sections {
            if let Some(contents) = s.pointer("/musicShelfRenderer/contents").and_then(|v| v.as_array()) {
                raw_items.extend(contents.clone());
            }
        }
    }

    for item in raw_items {
        if let Some(renderer) = item.get("musicResponsiveListItemRenderer") {
            if let Some(mut track) = parse_music_responsive_item(renderer) {
                if track.album.is_empty() {
                    track.album = album_title.clone();
                }
                if track.artist == "Artista Desconhecido" {
                    track.artist = album_artist.clone();
                }
                if track.thumbnail_url.is_empty() && !header_thumb.is_empty() {
                    track.thumbnail_url = header_thumb.clone();
                }
                tracks.push(track);
            }
        }
    }

    (album_title, tracks)
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

    #[test]
    fn test_parse_album_item() {
        let raw = json!({
            "navigationEndpoint": {
                "browseEndpoint": {
                    "browseId": "MPREb_12345"
                }
            },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Random Access Memories" }
                            ]
                        }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Álbum" },
                                { "text": " • " },
                                { "text": "Daft Punk" },
                                { "text": " • " },
                                { "text": "2013" }
                            ]
                        }
                    }
                }
            ],
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/album.jpg" }
                        ]
                    }
                }
            }
        });

        let album = parse_album_item(&raw).unwrap();
        assert_eq!(album.browse_id, "MPREb_12345");
        assert_eq!(album.title, "Random Access Memories");
        assert_eq!(album.artist, "Daft Punk");
        assert_eq!(album.year, "2013");
        assert_eq!(album.thumbnail_url, "https://lh3.googleusercontent.com/album.jpg");
    }

    #[test]
    fn test_parse_artist_item() {
        let raw = json!({
            "navigationEndpoint": {
                "browseEndpoint": {
                    "browseId": "UC_artist_123"
                }
            },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Daft Punk" }
                            ]
                        }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": {
                            "runs": [
                                { "text": "Artista" },
                                { "text": " • " },
                                { "text": "68,3 mi ouvintes mensais" }
                            ]
                        }
                    }
                }
            ],
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/artist.jpg" }
                        ]
                    }
                }
            }
        });

        let artist = parse_artist_item(&raw).unwrap();
        assert_eq!(artist.browse_id, "UC_artist_123");
        assert_eq!(artist.name, "Daft Punk");
        assert_eq!(artist.subscribers_text, "68,3 mi ouvintes mensais");
        assert_eq!(artist.thumbnail_url, "https://lh3.googleusercontent.com/artist.jpg");
    }
}
