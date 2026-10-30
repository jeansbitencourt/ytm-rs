use std::path::PathBuf;
use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub config_file: PathBuf,
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

        let runtime_base = dirs::runtime_dir()
            .unwrap_or_else(|| std::env::temp_dir());
        let runtime_dir = runtime_base.join("ytm-rs");

        let config_file = config_dir.join("config.toml");
        let auth_file = config_dir.join("auth.json");
        let db_file = data_dir.join("cache.db");
        let status_file = runtime_dir.join("status.json");
        let state_sock = PathBuf::from("/tmp/ytm-state.sock");
        let mpv_sock = PathBuf::from("/tmp/ytm-mpv.sock");

        let config = Self {
            config_dir,
            cache_dir: data_dir,
            runtime_dir,
            config_file,
            auth_file,
            db_file,
            status_file,
            state_sock,
            mpv_sock,
        };

        config.ensure_dirs()?;
        Ok(config)
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
