use crate::i18n::LanguagePreference;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

        let runtime_base = dirs::runtime_dir().unwrap_or_else(|| std::env::temp_dir());
        let runtime_dir = runtime_base.join("ytm-rs");

        let config_file = config_dir.join("config.toml");
        let auth_file = config_dir.join("auth.json");
        let db_file = data_dir.join("cache.db");
        let status_file = runtime_dir.join("status.json");
        let state_sock = PathBuf::from("/tmp/ytm-state.sock");
        let mpv_sock = PathBuf::from("/tmp/ytm-mpv.sock");

        let settings = Self::load_settings_file(&config_file);
        let config = Self {
            config_dir,
            cache_dir: data_dir,
            runtime_dir,
            config_file,
            settings,
            auth_file,
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

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("Failed to create config dir: {:?}", self.config_dir))?;
        std::fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("Failed to create cache dir: {:?}", self.cache_dir))?;
        std::fs::create_dir_all(&self.runtime_dir)
            .with_context(|| format!("Failed to create runtime dir: {:?}", self.runtime_dir))?;
        Ok(())
    }
}
