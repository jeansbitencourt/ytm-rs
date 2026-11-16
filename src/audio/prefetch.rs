use crate::cache::Database;
use crate::config::{AppConfig, PlaybackAuthSource};
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

const YT_DLP_TIMEOUT: Duration = Duration::from_secs(45);
const STREAM_URL_MIN_AGE: Duration = Duration::from_secs(5);
const STREAM_URL_CACHE_TTL_SECS: u64 = 14_400;
const PREFERRED_FORMAT: &str = "251/bestaudio[acodec=opus]/bestaudio";

#[derive(Debug, Clone, PartialEq)]
pub struct AvailableAudioFormat {
    pub id: String,
    pub extension: String,
    pub codec: String,
    pub bitrate_kbps: Option<f64>,
    pub sample_rate_hz: Option<u64>,
}

/// Direct media data is deliberately kept out of logs and persistent state for
/// authenticated playback. The URL and headers are temporary transport secrets.
pub struct ResolvedMedia {
    pub auth_source: PlaybackAuthSource,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub format: String,
    pub expires_at: Option<u64>,
    pub wait_before_use: Duration,
}

impl std::fmt::Debug for ResolvedMedia {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedMedia")
            .field("url", &"[redacted]")
            .field("headers", &"[redacted]")
            .field("format", &self.format)
            .field("expires_at", &self.expires_at)
            .field("wait_before_use", &self.wait_before_use)
            .finish()
    }
}

pub async fn resolve_stream_url(
    video_id: &str,
    db: &Arc<Mutex<Database>>,
    config: &AppConfig,
) -> Result<ResolvedMedia> {
    // Read this on every resolution so browser/profile changes and logout take
    // effect in the already-running daemon.
    let playback_auth = config.load_playback_auth()?;
    let authenticated = playback_auth.source.is_enabled();

    // The persistent cache contains only legacy anonymous URLs. Never reuse it
    // when a playback credential source has been selected.
    if !authenticated {
        let cached = {
            let db_guard = db.lock().await;
            db_guard.get_stream_url_with_age(video_id).ok().flatten()
        };
        if let Some((cached_url, age_secs)) = cached {
            debug!("Anonymous stream URL cache hit for video_id: {}", video_id);
            return Ok(ResolvedMedia {
                auth_source: PlaybackAuthSource::Disabled,
                url: cached_url,
                headers: Vec::new(),
                format: "cached-anonymous".to_string(),
                expires_at: None,
                wait_before_use: STREAM_URL_MIN_AGE.saturating_sub(Duration::from_secs(age_secs)),
            });
        }
    }

    info!(
        "Resolving {} stream via yt-dlp for video_id: {}",
        if authenticated {
            "configured"
        } else {
            "anonymous"
        },
        video_id
    );
    let result = query_ytdlp_metadata(
        video_id,
        config,
        &playback_auth.source,
        Some(PREFERRED_FORMAT),
    )
    .await?;
    let url = result
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .context("yt-dlp did not provide a direct media URL")?
        .to_string();
    let format = result
        .get("format_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| PREFERRED_FORMAT.to_string());
    let headers = parse_http_headers(result.get("http_headers"))?;
    let expires_at = expiration_timestamp(&url);

    if !authenticated {
        let now = unix_now();
        let ttl = expires_at
            .map(|expiry| expiry.saturating_sub(now).min(STREAM_URL_CACHE_TTL_SECS))
            .unwrap_or(STREAM_URL_CACHE_TTL_SECS);
        if ttl > 0 {
            let saved = {
                let db_guard = db.lock().await;
                db_guard.save_stream_url(video_id, &url, ttl)
            };
            if let Err(error) = saved {
                warn!(
                    "Could not cache anonymous stream URL for {}: {}",
                    video_id, error
                );
            }
        }
    }

    Ok(ResolvedMedia {
        auth_source: playback_auth.source,
        url,
        headers,
        format,
        expires_at,
        wait_before_use: STREAM_URL_MIN_AGE,
    })
}

pub async fn inspect_stream_formats(
    video_id: &str,
    config: &AppConfig,
) -> Result<Vec<AvailableAudioFormat>> {
    validate_video_id(video_id)?;
    let playback_auth = config.load_playback_auth()?;
    if !playback_auth.source.is_enabled() {
        anyhow::bail!(
            "Configure explicitamente uma fonte de credenciais de reprodução antes da consulta"
        );
    }

    // No format selector means yt-dlp returns its available formats in metadata.
    // The result is filtered in memory so URLs and HTTP headers are never printed.
    let metadata = query_ytdlp_metadata(video_id, config, &playback_auth.source, None).await?;
    parse_available_audio_formats(&metadata)
}

fn parse_available_audio_formats(metadata: &Value) -> Result<Vec<AvailableAudioFormat>> {
    let formats = metadata
        .get("formats")
        .and_then(Value::as_array)
        .context("yt-dlp não informou a lista de formatos disponíveis")?;
    let mut audio_formats = formats
        .iter()
        .filter_map(|format| {
            let codec = format.get("acodec")?.as_str()?;
            if codec.is_empty() || codec == "none" {
                return None;
            }
            Some(AvailableAudioFormat {
                id: format.get("format_id")?.as_str()?.to_string(),
                extension: format
                    .get("ext")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                codec: codec.to_string(),
                bitrate_kbps: format
                    .get("abr")
                    .and_then(Value::as_f64)
                    .or_else(|| format.get("tbr").and_then(Value::as_f64)),
                sample_rate_hz: format.get("asr").and_then(Value::as_u64),
            })
        })
        .collect::<Vec<_>>();
    audio_formats.sort_by(|left, right| left.id.cmp(&right.id));
    if audio_formats.is_empty() {
        anyhow::bail!("yt-dlp não informou formatos de áudio");
    }
    Ok(audio_formats)
}

fn build_ytdlp_command(
    video_id: &str,
    config: &AppConfig,
    source: &PlaybackAuthSource,
    format: Option<&str>,
) -> Command {
    let mut command = Command::new("yt-dlp");
    command
        .arg("--ignore-config")
        .arg("--dump-single-json")
        .arg("--skip-download")
        .arg("--no-warnings")
        .arg("--no-playlist");
    if let Some(format) = format {
        command.arg("--format").arg(format);
    }

    match source {
        PlaybackAuthSource::Disabled => {}
        PlaybackAuthSource::Browser { browser, profile } => {
            command.arg("--cookies-from-browser");
            let browser_spec = match profile {
                Some(profile) => format!("{browser}:{profile}"),
                None => browser.clone(),
            };
            command.arg(browser_spec);
        }
        PlaybackAuthSource::NetscapeCookieFile => {
            command.arg("--cookies").arg(&config.playback_cookies_file);
        }
    }
    if source.is_enabled() {
        // Keep an authenticated diagnostic to a single extraction attempt.
        command
            .arg("--retries")
            .arg("0")
            .arg("--extractor-retries")
            .arg("0")
            .arg("--fragment-retries")
            .arg("0");
    }
    let watch_url = format!("https://music.youtube.com/watch?v={video_id}");
    command.arg("--").arg(watch_url);
    command
}

async fn query_ytdlp_metadata(
    video_id: &str,
    config: &AppConfig,
    source: &PlaybackAuthSource,
    format: Option<&str>,
) -> Result<Value> {
    let output = timeout(
        YT_DLP_TIMEOUT,
        build_ytdlp_command(video_id, config, source, format)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "Tempo limite do yt-dlp após {} segundos",
            YT_DLP_TIMEOUT.as_secs()
        )
    })??;

    if !output.status.success() {
        // yt-dlp diagnostics can contain signed URLs or other session details.
        // Do not forward stderr into logs, status JSON, or the TUI.
        let mode = if source.is_enabled() {
            "as credenciais de reprodução configuradas"
        } else {
            "a reprodução anônima"
        };
        anyhow::bail!(
            "yt-dlp não conseguiu resolver {mode} (código de saída {})",
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string())
        );
    }
    serde_json::from_slice(&output.stdout).context("yt-dlp retornou metadados de mídia inválidos")
}

fn validate_video_id(video_id: &str) -> Result<()> {
    if video_id.len() != 11
        || !video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        anyhow::bail!("O ID do vídeo deve ter 11 caracteres válidos do YouTube");
    }
    Ok(())
}

fn parse_http_headers(value: Option<&Value>) -> Result<Vec<(String, String)>> {
    let Some(headers) = value.and_then(Value::as_object) else {
        return Ok(Vec::new());
    };

    let mut result = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let Some(value) = value.as_str() else {
            continue;
        };
        if name.chars().any(char::is_control) || value.chars().any(char::is_control) {
            anyhow::bail!("yt-dlp returned an invalid HTTP header");
        }
        result.push((name.clone(), value.to_string()));
    }
    Ok(result)
}

fn expiration_timestamp(url: &str) -> Option<u64> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(key, _)| key == "expire")
        .and_then(|(_, value)| value.parse().ok())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn spawn_prefetch(
    video_id: String,
    db: Arc<Mutex<Database>>,
    config: AppConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        debug!("Starting async prefetch worker for video_id: {}", video_id);
        match resolve_stream_url(&video_id, &db, &config).await {
            Ok(resolved) => {
                // Keep metadata available to this scope only. Never log URLs,
                // HTTP headers, or format payloads from authenticated extraction.
                debug!("Prefetch completed for {} ({})", video_id, resolved.format);
            }
            Err(error) => error!("Prefetch failed for {}: {}", video_id, error),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UserSettings;
    use std::path::PathBuf;

    fn test_config() -> AppConfig {
        let root = PathBuf::from("/tmp/ytm-rs-yt-dlp-args-test");
        AppConfig {
            config_dir: root.clone(),
            cache_dir: root.clone(),
            runtime_dir: root.clone(),
            config_file: root.join("config.toml"),
            settings: UserSettings::default(),
            auth_file: root.join("auth.json"),
            playback_auth_file: root.join("playback-auth.json"),
            playback_cookies_file: root.join("playback-cookies.txt"),
            db_file: root.join("cache.db"),
            status_file: root.join("status.json"),
            state_sock: root.join("state.sock"),
            mpv_sock: root.join("mpv.sock"),
        }
    }

    #[test]
    fn parses_stream_expiration_without_exposing_the_url() {
        assert_eq!(
            expiration_timestamp("https://example.test/audio?expire=1893456000&sig=private"),
            Some(1_893_456_000)
        );
        assert_eq!(expiration_timestamp("not a URL"), None);
    }

    #[test]
    fn parses_transport_headers_and_rejects_control_characters() {
        let headers = serde_json::json!({
            "User-Agent": "ytm-rs-test",
            "Cookie": "session=secret",
        });
        let parsed = parse_http_headers(Some(&headers)).unwrap();
        assert_eq!(parsed.len(), 2);

        let invalid = serde_json::json!({ "Cookie": "session=secret\r\nInjected: yes" });
        assert!(parse_http_headers(Some(&invalid)).is_err());
    }

    #[test]
    fn resolved_media_debug_redacts_transport_secrets() {
        let media = ResolvedMedia {
            auth_source: PlaybackAuthSource::NetscapeCookieFile,
            url: "https://example.test/audio?sig=private".to_string(),
            headers: vec![("Cookie".to_string(), "session=private".to_string())],
            format: "251".to_string(),
            expires_at: None,
            wait_before_use: Duration::ZERO,
        };
        let debug = format!("{media:?}");
        assert!(!debug.contains("sig=private"));
        assert!(!debug.contains("session=private"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn inspection_parses_only_audio_format_metadata() {
        let metadata = serde_json::json!({
            "formats": [
                {
                    "format_id": "251",
                    "url": "https://example.test/audio?sig=private",
                    "ext": "webm",
                    "acodec": "opus",
                    "abr": 129.0,
                    "asr": 48000,
                    "vcodec": "none"
                },
                {"format_id": "137", "acodec": "none"}
            ]
        });
        let formats = parse_available_audio_formats(&metadata).unwrap();
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].id, "251");
        assert_eq!(formats[0].codec, "opus");
        assert_eq!(formats[0].bitrate_kbps, Some(129.0));
        assert!(!format!("{formats:?}").contains("sig=private"));
    }

    #[test]
    fn authenticated_extraction_ignores_global_config_and_disables_retries() {
        let source = PlaybackAuthSource::Browser {
            browser: "chrome".to_string(),
            profile: None,
        };
        let command = build_ytdlp_command("dQw4w9WgXcQ", &test_config(), &source, None);
        let args = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for required in [
            "--ignore-config",
            "--dump-single-json",
            "--skip-download",
            "--cookies-from-browser",
            "chrome",
            "--retries",
            "--extractor-retries",
            "--fragment-retries",
        ] {
            assert!(args.iter().any(|argument| argument == required));
        }
        assert_eq!(args.iter().filter(|argument| *argument == "0").count(), 3);
        assert!(!args.contains(&"--format".to_string()));
    }

    #[test]
    fn video_id_validation_rejects_non_youtube_input() {
        assert!(validate_video_id("dQw4w9WgXcQ").is_ok());
        assert!(validate_video_id("--cookies-from-browser").is_err());
        assert!(validate_video_id("short").is_err());
    }
}
