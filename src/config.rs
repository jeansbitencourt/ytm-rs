use crate::i18n::LanguagePreference;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlaybackAuthSource {
    #[default]
    Disabled,
    Browser {
        browser: String,
        #[serde(default)]
        profile: Option<String>,
    },
    NetscapeCookieFile,
}

impl PlaybackAuthSource {
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub fn description(&self) -> String {
        match self {
            Self::Disabled => "anônimo".to_string(),
            Self::Browser { browser, profile } => match profile {
                Some(profile) => format!("cookies do navegador {browser} (perfil {profile})"),
                None => format!("cookies do navegador {browser}"),
            },
            Self::NetscapeCookieFile => "arquivo Netscape importado".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlaybackAuthSettings {
    #[serde(default)]
    pub source: PlaybackAuthSource,
    #[serde(default)]
    pub prefetch_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub general: GeneralSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(default)]
    pub language: LanguagePreference,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            general: GeneralSettings::default(),
        }
    }
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            language: LanguagePreference::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub config_file: PathBuf,
    pub settings: UserSettings,
    pub auth_file: PathBuf,
    pub playback_auth_file: PathBuf,
    pub playback_cookies_file: PathBuf,
    pub db_file: PathBuf,
    pub status_file: PathBuf,
    pub state_sock: PathBuf,
    pub mpv_sock: PathBuf,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        let config_dir = dirs::config_dir()
            .map(|p| p.join("ytm-rs"))
            .unwrap_or_else(|| home.join(".config").join("ytm-rs"));

        let data_dir = dirs::data_local_dir()
            .map(|p| p.join("ytm-rs"))
            .unwrap_or_else(|| home.join(".local").join("share").join("ytm-rs"));

        let runtime_dir = dirs::runtime_dir()
            .filter(|path| is_private_directory(path))
            .map(|path| path.join("ytm-rs"))
            .unwrap_or_else(|| fallback_runtime_dir(&home));

        let config_file = config_dir.join("config.toml");
        let auth_file = config_dir.join("auth.json");
        let playback_auth_file = config_dir.join("playback-auth.json");
        let playback_cookies_file = config_dir.join("playback-cookies.txt");
        let db_file = data_dir.join("cache.db");
        let status_file = runtime_dir.join("status.json");
        let state_sock = runtime_dir.join("state.sock");
        let mpv_sock = runtime_dir.join("mpv.sock");

        let settings = Self::load_settings_file(&config_file);
        let config = Self {
            config_dir,
            cache_dir: data_dir,
            runtime_dir,
            config_file,
            settings,
            auth_file,
            playback_auth_file,
            playback_cookies_file,
            db_file,
            status_file,
            state_sock,
            mpv_sock,
        };

        config.ensure_dirs()?;
        Ok(config)
    }

    fn load_settings_file(path: &PathBuf) -> UserSettings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| toml::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &UserSettings) -> Result<()> {
        let content = toml::to_string_pretty(settings).context("Failed to serialize settings")?;
        let tmp_path = self.config_file.with_extension("toml.tmp");
        std::fs::write(&tmp_path, content)
            .with_context(|| format!("Failed to write settings: {:?}", tmp_path))?;
        std::fs::rename(&tmp_path, &self.config_file)
            .with_context(|| format!("Failed to replace settings: {:?}", self.config_file))?;
        Ok(())
    }

    pub fn load_playback_auth(&self) -> Result<PlaybackAuthSettings> {
        if !self.playback_auth_file.exists() {
            return Ok(PlaybackAuthSettings::default());
        }

        let content = fs::read_to_string(&self.playback_auth_file)
            .with_context(|| format!("Could not read playback authentication settings"))?;
        let settings: PlaybackAuthSettings = serde_json::from_str(&content)
            .context("Playback authentication settings are invalid")?;
        self.validate_playback_auth(&settings)?;
        Ok(settings)
    }

    pub fn save_playback_auth(&self, settings: &PlaybackAuthSettings) -> Result<()> {
        self.validate_playback_auth(settings)?;
        self.ensure_dirs()?;

        let content = serde_json::to_vec_pretty(settings)?;
        let tmp_path = self
            .playback_auth_file
            .with_extension(format!("json.tmp.{}", unique_suffix()));
        write_private_file(&tmp_path, &content)?;
        if let Err(error) = fs::rename(&tmp_path, &self.playback_auth_file) {
            let _ = fs::remove_file(&tmp_path);
            return Err(error).with_context(|| "Could not save playback authentication settings");
        }

        if !matches!(settings.source, PlaybackAuthSource::NetscapeCookieFile) {
            remove_if_present(&self.playback_cookies_file)?;
        }
        Ok(())
    }

    pub fn import_playback_cookies(&self, source: &Path) -> Result<()> {
        let contents = fs::read(source)
            .with_context(|| format!("Could not read cookie file: {}", source.display()))?;
        validate_netscape_cookie_file(&contents)?;
        self.ensure_dirs()?;

        let tmp_path = self
            .playback_cookies_file
            .with_extension(format!("txt.tmp.{}", unique_suffix()));
        write_private_file(&tmp_path, &contents)?;
        if let Err(error) = fs::rename(&tmp_path, &self.playback_cookies_file) {
            let _ = fs::remove_file(&tmp_path);
            return Err(error).with_context(|| "Could not install the private cookie file");
        }

        self.save_playback_auth(&PlaybackAuthSettings {
            source: PlaybackAuthSource::NetscapeCookieFile,
            prefetch_enabled: false,
        })
    }

    pub fn clear_playback_auth(&self) -> Result<()> {
        self.save_playback_auth(&PlaybackAuthSettings::default())
    }

    fn validate_playback_auth(&self, settings: &PlaybackAuthSettings) -> Result<()> {
        match &settings.source {
            PlaybackAuthSource::Disabled => Ok(()),
            PlaybackAuthSource::Browser { browser, profile } => {
                const SUPPORTED_BROWSERS: &[&str] = &[
                    "brave", "chrome", "chromium", "edge", "firefox", "opera", "safari", "vivaldi",
                    "whale",
                ];
                if !SUPPORTED_BROWSERS.contains(&browser.as_str()) {
                    anyhow::bail!("Unsupported browser for yt-dlp cookie extraction");
                }
                if profile.as_deref().is_some_and(|value| {
                    value.trim().is_empty() || value.chars().any(char::is_control)
                }) {
                    anyhow::bail!(
                        "Browser profile must be non-empty text without control characters"
                    );
                }
                Ok(())
            }
            PlaybackAuthSource::NetscapeCookieFile => {
                let metadata = fs::symlink_metadata(&self.playback_cookies_file)
                    .context("The imported Netscape cookie file is missing")?;
                if !metadata.file_type().is_file() {
                    anyhow::bail!("The imported cookie path must be a regular file");
                }
                ensure_private_permissions(&metadata)?;
                let contents = fs::read(&self.playback_cookies_file)
                    .context("Could not read the imported Netscape cookie file")?;
                validate_netscape_cookie_file(&contents)
            }
        }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("Failed to create config dir: {:?}", self.config_dir))?;
        std::fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("Failed to create cache dir: {:?}", self.cache_dir))?;
        if self.runtime_dir.parent().is_some_and(is_private_directory) {
            std::fs::create_dir_all(&self.runtime_dir)
                .with_context(|| format!("Failed to create runtime dir: {:?}", self.runtime_dir))?;
        } else {
            create_private_runtime_dir(&self.runtime_dir)?;
        }
        Ok(())
    }
}

fn validate_netscape_cookie_file(contents: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(contents).context("Cookie file must be UTF-8 text")?;
    let has_header = text.lines().any(|line| {
        line.trim() == "# Netscape HTTP Cookie File" || line.trim() == "# HTTP Cookie File"
    });
    let has_cookie = text
        .lines()
        .filter(|line| {
            !line.trim().is_empty() && (!line.starts_with('#') || line.starts_with("#HttpOnly_"))
        })
        .any(|line| line.split('\t').count() >= 7);
    if !has_header || !has_cookie {
        anyhow::bail!("Expected a Netscape-format cookie file with its standard header");
    }
    Ok(())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("Could not create private file: {}", path.display()))?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(error)
            .with_context(|| format!("Could not write private file: {}", path.display()));
    }
    Ok(())
}

fn ensure_private_permissions(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!(
                "Imported cookie file permissions must be restricted to the current user"
            );
        }
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("Could not remove file: {}", path.display()))
        }
    }
}

#[cfg(unix)]
fn is_private_directory(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_private_directory(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
fn fallback_runtime_dir(home: &Path) -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let uid = fs::metadata(home)
        .map(|metadata| metadata.uid())
        .unwrap_or_else(|_| std::process::id());
    std::env::temp_dir().join(format!("ytm-rs-runtime-{uid}"))
}

#[cfg(not(unix))]
fn fallback_runtime_dir(_home: &Path) -> PathBuf {
    std::env::temp_dir().join("ytm-rs-runtime")
}

#[cfg(unix)]
fn create_private_runtime_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to create private runtime dir: {:?}", path));
        }
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Could not inspect runtime dir: {:?}", path))?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("Runtime directory must be a private directory: {:?}", path);
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_runtime_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("Failed to create runtime dir: {:?}", path))
}

fn unique_suffix() -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{timestamp}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> (AppConfig, PathBuf) {
        let suffix = unique_suffix();
        let root = std::env::temp_dir().join(format!("ytm-rs-config-test-{suffix}"));
        let config_dir = root.join("config");
        let runtime_dir = root.join("runtime");
        let cache_dir = root.join("cache");
        let config = AppConfig {
            config_dir: config_dir.clone(),
            cache_dir,
            runtime_dir,
            config_file: config_dir.join("config.toml"),
            settings: UserSettings::default(),
            auth_file: config_dir.join("auth.json"),
            playback_auth_file: config_dir.join("playback-auth.json"),
            playback_cookies_file: config_dir.join("playback-cookies.txt"),
            db_file: root.join("cache.db"),
            status_file: root.join("status.json"),
            state_sock: root.join("state.sock"),
            mpv_sock: root.join("mpv.sock"),
        };
        (config, root)
    }

    #[test]
    fn imports_netscape_cookies_privately_and_clears_them() {
        let (config, root) = test_config();
        let source = root.join("source-cookies.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &source,
            "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsecret-value\n",
        )
        .unwrap();

        config.import_playback_cookies(&source).unwrap();
        assert_eq!(
            config.load_playback_auth().unwrap().source,
            PlaybackAuthSource::NetscapeCookieFile
        );
        assert_eq!(
            fs::read(&config.playback_cookies_file).unwrap(),
            fs::read(&source).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&config.playback_cookies_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        config.clear_playback_auth().unwrap();
        assert!(!config.playback_cookies_file.exists());
        assert_eq!(
            config.load_playback_auth().unwrap(),
            PlaybackAuthSettings::default()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_cookie_headers_as_netscape_files() {
        let (config, root) = test_config();
        fs::create_dir_all(&root).unwrap();
        let source = root.join("header.txt");
        fs::write(&source, "SID=secret-value; SAPISID=another-secret").unwrap();
        assert!(config.import_playback_cookies(&source).is_err());
        assert!(!config.playback_cookies_file.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unsupported_browser_sources() {
        let (config, root) = test_config();
        let settings = PlaybackAuthSettings {
            source: PlaybackAuthSource::Browser {
                browser: "unknown".to_string(),
                profile: None,
            },
            prefetch_enabled: false,
        };
        assert!(config.save_playback_auth(&settings).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
