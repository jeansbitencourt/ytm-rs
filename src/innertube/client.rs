use anyhow::{Context, Result};
use reqwest::header::{
    HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT,
};
use serde_json::{json, Value};
use tracing::warn;

use super::auth::AuthConfig;
pub use super::parsers::*;
use super::{AlbumSummary, ArtistSummary, PlaylistSummary, SearchFilter, Track, UserProfile};
use crate::config::AppConfig;
use crate::i18n::{I18n, LanguagePreference};

const INNERTUBE_BASE_URL: &str = "https://music.youtube.com/youtubei/v1";
const YOUTUBE_BASE_URL: &str = "https://www.youtube.com/youtubei/v1";
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
        let preference = AppConfig::load()
            .map(|config| config.settings.general.language)
            .unwrap_or(LanguagePreference::Auto);
        let language = I18n::new(preference).language.code();
        json!({
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": "1.20261027.10.00",
                "hl": language,
                "gl": "BR"
            }
        })
    }

    fn tv_context() -> Value {
        json!({
            "client": {
                "clientName": "TVHTML5",
                "clientVersion": "7.20230405.08.01",
                "theme": "CLASSIC"
            }
        })
    }

    fn build_web_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ORIGIN, HeaderValue::from_static(WEB_REMIX_ORIGIN));
        headers.insert("X-Origin", HeaderValue::from_static(WEB_REMIX_ORIGIN));
        headers.insert(
            REFERER,
            HeaderValue::from_static("https://music.youtube.com/"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));
        headers.insert("X-Goog-AuthUser", HeaderValue::from_static("0"));

        if let Some(cookies) = &self.auth.cookies {
            if let Ok(val) = HeaderValue::from_str(cookies) {
                headers.insert(COOKIE, val);
            }
        }

        if let Some(auth_hdr) = self.auth.get_sapisid_auth_header(WEB_REMIX_ORIGIN) {
            if let Ok(val) = HeaderValue::from_str(&auth_hdr) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        headers
    }

    fn build_tv_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));

        if let Some(token) = &self.auth.oauth_token {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", token)) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        headers
    }

    pub async fn ensure_valid_token(&mut self, auth_path: Option<&std::path::Path>) -> Result<()> {
        if self.auth.is_token_expired() {
            if let Some(ref_tok) = &self.auth.refresh_token {
                let tok = super::oauth::refresh_access_token(
                    super::oauth::DEFAULT_CLIENT_ID,
                    super::oauth::DEFAULT_CLIENT_SECRET,
                    ref_tok,
                )
                .await?;

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                self.auth.oauth_token = Some(tok.access_token);
                self.auth.token_expiry = Some(now + tok.expires_in);
                if let Some(path) = auth_path {
                    let _ = self.auth.save(path);
                }
            }
        }
        Ok(())
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let url = format!("{}/search", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "query": query,
            // Filter to Songs / Tracks when possible
            "params": "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D"
        });

        let headers = self.build_web_headers();
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send search request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse search response JSON")?;
        let tracks = parse_search_results(&body);
        if tracks.is_empty() {
            // Fallback to search without params
            let payload_fallback = json!({
                "context": Self::default_context(),
                "query": query,
            });
            let headers = self.build_web_headers();
            if let Ok(r) = self
                .http
                .post(&url)
                .headers(headers)
                .json(&payload_fallback)
                .send()
                .await
            {
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

        let headers = self.build_web_headers();
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send radio request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse radio response JSON")?;
        Ok(parse_radio_results(&body))
    }

    pub async fn browse_continuation(&self, continuation: &str) -> Result<Value> {
        let (url, payload, headers) = if self.auth.has_oauth() {
            (
                format!("{}/browse", YOUTUBE_BASE_URL),
                json!({
                    "context": Self::tv_context(),
                    "continuation": continuation
                }),
                self.build_tv_headers(),
            )
        } else {
            (
                format!("{}/browse", INNERTUBE_BASE_URL),
                json!({
                    "context": Self::default_context(),
                    "continuation": continuation
                }),
                self.build_web_headers(),
            )
        };

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send continuation request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse continuation response JSON")?;
        Ok(body)
    }

    pub async fn get_liked_music(&self) -> Result<Vec<Track>> {
        let (url, payload, headers) = if self.auth.has_oauth() {
            (
                format!("{}/browse", YOUTUBE_BASE_URL),
                json!({
                    "context": Self::tv_context(),
                    "browseId": "FEmusic_liked_videos"
                }),
                self.build_tv_headers(),
            )
        } else {
            (
                format!("{}/browse", INNERTUBE_BASE_URL),
                json!({
                    "context": Self::default_context(),
                    "browseId": "FEmusic_liked_videos"
                }),
                self.build_web_headers(),
            )
        };

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send browse request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse browse response JSON")?;
        let mut tracks = parse_playlist_results(&body);
        let mut continuation_token = extract_browse_continuation_token(&body);

        let mut pages = 1;
        while let Some(tok) = continuation_token {
            if pages >= 30 {
                break;
            }
            if let Ok(c_body) = self.browse_continuation(&tok).await {
                let (c_tracks, next_tok) = parse_continuation_response(&c_body);
                if c_tracks.is_empty() {
                    break;
                }
                tracks.extend(c_tracks);
                continuation_token = next_tok;
                pages += 1;
            } else {
                break;
            }
        }

        for t in &mut tracks {
            t.is_liked = true;
        }

        Ok(tracks)
    }

    pub async fn like_track(&self, video_id: &str, like: bool) -> Result<()> {
        let endpoint = if like { "like/like" } else { "like/removelike" };
        let (url, payload, headers) = if self.auth.has_oauth() {
            (
                format!("{}/{}", YOUTUBE_BASE_URL, endpoint),
                json!({
                    "context": Self::tv_context(),
                    "target": {
                        "videoId": video_id
                    }
                }),
                self.build_tv_headers(),
            )
        } else {
            (
                format!("{}/{}", INNERTUBE_BASE_URL, endpoint),
                json!({
                    "context": Self::default_context(),
                    "target": {
                        "videoId": video_id
                    }
                }),
                self.build_web_headers(),
            )
        };

        let resp = self
            .http
            .post(&url)
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

        let headers = self.build_web_headers();
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send album search request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse album search response JSON")?;
        Ok(parse_album_search_results(&body))
    }

    pub async fn search_artists(&self, query: &str) -> Result<Vec<ArtistSummary>> {
        let url = format!("{}/search", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "query": query,
            "params": SearchFilter::Artists.params()
        });

        let headers = self.build_web_headers();
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send artist search request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse artist search response JSON")?;
        Ok(parse_artist_search_results(&body))
    }

    pub async fn get_album_tracks(&self, browse_id: &str) -> Result<(String, Vec<Track>)> {
        let url = format!("{}/browse", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "browseId": browse_id
        });

        let headers = self.build_web_headers();
        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send album browse request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse album browse response JSON")?;
        Ok(parse_album_browse_tracks(&body))
    }

    pub async fn get_artist_tracks(&self, browse_id: &str) -> Result<(String, Vec<Track>)> {
        let url = format!("{}/browse", INNERTUBE_BASE_URL);
        let payload = json!({
            "context": Self::default_context(),
            "browseId": browse_id
        });

        let headers = self.build_web_headers();
        let resp = self
            .http
            .post(&url)
            .headers(headers.clone())
            .json(&payload)
            .send()
            .await
            .context("Failed to send artist browse request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse artist browse response JSON")?;

        let artist_name = body
            .pointer("/header/musicImmersiveHeaderRenderer/title/runs/0/text")
            .or_else(|| body.pointer("/header/musicVisualHeaderRenderer/title/runs/0/text"))
            .and_then(|t| t.as_str())
            .unwrap_or("Artista")
            .to_string();

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
            if let Ok(r2) = self
                .http
                .post(&url)
                .headers(headers)
                .json(&payload2)
                .send()
                .await
            {
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

        // Fallback: extract tracks from initial artist shelves
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

    /// Fetches the authenticated user's profile (name, handle, avatar URL)
    pub async fn get_user_profile(&self) -> Result<Option<UserProfile>> {
        if self.auth.has_oauth() {
            let url = format!("{}/guide", YOUTUBE_BASE_URL);
            let payload = json!({
                "context": Self::tv_context()
            });

            let headers = self.build_tv_headers();
            let resp = self
                .http
                .post(&url)
                .headers(headers)
                .json(&payload)
                .send()
                .await
                .context("Failed to send guide request to Innertube")?;

            let body: Value = resp
                .json()
                .await
                .context("Failed to parse guide response JSON")?;
            Ok(parse_user_profile(&body))
        } else {
            let url = format!("{}/account/account_menu", INNERTUBE_BASE_URL);
            let payload = json!({
                "context": Self::default_context()
            });

            let headers = self.build_web_headers();
            let resp = self
                .http
                .post(&url)
                .headers(headers)
                .json(&payload)
                .send()
                .await
                .context("Failed to send account_menu request to Innertube")?;

            let body: Value = resp
                .json()
                .await
                .context("Failed to parse account_menu response JSON")?;
            Ok(parse_user_profile(&body))
        }
    }

    /// Fetches the authenticated user's playlists from their library
    pub async fn get_user_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        let (url, payload, headers) = if self.auth.has_oauth() {
            (
                format!("{}/browse", YOUTUBE_BASE_URL),
                json!({
                    "context": Self::tv_context(),
                    "browseId": "FEmusic_liked_playlists"
                }),
                self.build_tv_headers(),
            )
        } else {
            (
                format!("{}/browse", INNERTUBE_BASE_URL),
                json!({
                    "context": Self::default_context(),
                    "browseId": "FEmusic_liked_playlists"
                }),
                self.build_web_headers(),
            )
        };

        let resp = self
            .http
            .post(&url)
            .headers(headers.clone())
            .json(&payload)
            .send()
            .await
            .context("Failed to send library playlists request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse library playlists response JSON")?;
        let mut list = parse_library_playlists(&body);
        if list.is_empty() && !self.auth.has_oauth() {
            // Fallback to FEmusic_library_landing for Web mode
            let payload_fallback = json!({
                "context": Self::default_context(),
                "browseId": "FEmusic_library_landing"
            });
            if let Ok(r) = self
                .http
                .post(&url)
                .headers(headers)
                .json(&payload_fallback)
                .send()
                .await
            {
                if let Ok(b) = r.json::<Value>().await {
                    list = parse_library_playlists(&b);
                }
            }
        }
        Ok(list)
    }

    /// Fetches tracks from a specific user playlist
    pub async fn get_playlist_tracks(&self, browse_id: &str) -> Result<(String, Vec<Track>)> {
        let clean_id = if browse_id.starts_with("VL") || browse_id.starts_with("MPRE") {
            browse_id.to_string()
        } else {
            format!("VL{}", browse_id)
        };

        let (url, payload, headers) = if self.auth.has_oauth() {
            (
                format!("{}/browse", YOUTUBE_BASE_URL),
                json!({
                    "context": Self::tv_context(),
                    "browseId": clean_id
                }),
                self.build_tv_headers(),
            )
        } else {
            (
                format!("{}/browse", INNERTUBE_BASE_URL),
                json!({
                    "context": Self::default_context(),
                    "browseId": clean_id
                }),
                self.build_web_headers(),
            )
        };

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .context("Failed to send playlist browse request to Innertube")?;

        let body: Value = resp
            .json()
            .await
            .context("Failed to parse playlist browse response JSON")?;

        let title = body
            .pointer("/contents/tvBrowseRenderer/content/tvSurfaceContentRenderer/content/twoColumnRenderer/leftColumn/entityMetadataRenderer/title/simpleText")
            .or_else(|| body.pointer("/contents/tvBrowseRenderer/content/tvSurfaceContentRenderer/content/twoColumnRenderer/leftColumn/entityMetadataRenderer/title/runs/0/text"))
            .or_else(|| body.pointer("/header/musicResponsiveHeaderRenderer/title/runs/0/text"))
            .or_else(|| body.pointer("/header/musicDetailHeaderRenderer/title/runs/0/text"))
            .or_else(|| body.pointer("/header/musicEditablePlaylistDetailHeaderRenderer/header/musicDetailHeaderRenderer/title/runs/0/text"))
            .and_then(|t| t.as_str())
            .unwrap_or("Playlist")
            .to_string();

        let tracks = parse_playlist_results(&body);
        let mut all_tracks = tracks;
        let mut continuation_token = extract_browse_continuation_token(&body);

        let mut pages = 1;
        while let Some(tok) = continuation_token {
            if pages >= 30 {
                break;
            }
            if let Ok(c_body) = self.browse_continuation(&tok).await {
                let (c_tracks, next_tok) = parse_continuation_response(&c_body);
                if c_tracks.is_empty() {
                    break;
                }
                all_tracks.extend(c_tracks);
                continuation_token = next_tok;
                pages += 1;
            } else {
                break;
            }
        }

        // Fallback for Web Remix if TVHTML5 returned 0 tracks
        if all_tracks.is_empty() && self.auth.has_oauth() && self.auth.cookies.is_some() {
            let web_url = format!("{}/browse", INNERTUBE_BASE_URL);
            let web_payload = json!({
                "context": Self::default_context(),
                "browseId": clean_id
            });
            if let Ok(resp2) = self
                .http
                .post(&web_url)
                .headers(self.build_web_headers())
                .json(&web_payload)
                .send()
                .await
            {
                if let Ok(body2) = resp2.json::<Value>().await {
                    let mut t2 = parse_playlist_results(&body2);
                    if !t2.is_empty() {
                        let mut cont2 = extract_browse_continuation_token(&body2);
                        let mut p = 1;
                        while let Some(tok2) = cont2 {
                            if p >= 30 {
                                break;
                            }
                            if let Ok(cb) = self.browse_continuation(&tok2).await {
                                let (ct, nt) = parse_continuation_response(&cb);
                                if ct.is_empty() {
                                    break;
                                }
                                t2.extend(ct);
                                cont2 = nt;
                                p += 1;
                            } else {
                                break;
                            }
                        }
                        return Ok((title, t2));
                    }
                }
            }
        }

        Ok((title, all_tracks))
    }
}
