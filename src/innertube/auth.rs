use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub cookies: Option<String>,
    pub sapisid: Option<String>,
    pub authorization: Option<String>,
    pub oauth_token: Option<String>,
}

impl AuthConfig {
    pub fn load(path: &Path) -> Self {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(auth) = serde_json::from_str::<AuthConfig>(&content) {
                    return auth;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.cookies.is_some() || self.sapisid.is_some() || self.oauth_token.is_some()
    }

    pub fn get_sapisid_auth_header(&self, origin: &str) -> Option<String> {
        let sapisid = self.sapisid.as_deref()?;
        Some(generate_sapisid_hash(sapisid, origin))
    }
}

pub fn generate_sapisid_hash(sapisid: &str, origin: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let input = format!("{} {} {}", now, sapisid, origin);
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    let hash = hex::encode(hasher.finalize());

    format!("SAPISIDHASH {}_{}", now, hash)
}

pub fn extract_sapisid_from_cookies(cookies: &str) -> Option<String> {
    for part in cookies.split(';') {
        let trimmed = part.trim();
        if let Some(s) = trimmed.strip_prefix("SAPISID=") {
            return Some(s.to_string());
        }
        if let Some(s) = trimmed.strip_prefix("__Secure-3PAPISID=") {
            return Some(s.to_string());
        }
    }
    let trimmed = cookies.trim();
    if !trimmed.contains(';') && !trimmed.contains('=') && trimmed.len() >= 10 {
        return Some(trimmed.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sapisid_hash() {
        let hash = generate_sapisid_hash("test_sapisid", "https://music.youtube.com");
        assert!(hash.starts_with("SAPISIDHASH "));
        assert!(hash.contains('_'));
    }

    #[test]
    fn test_extract_sapisid() {
        let cookies = "SID=123; SAPISID=abc_xyz_456; HSID=789";
        assert_eq!(extract_sapisid_from_cookies(cookies), Some("abc_xyz_456".to_string()));
    }
}
