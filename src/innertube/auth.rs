use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub cookies: Option<String>,
    pub sapisid: Option<String>,
    pub authorization: Option<String>,
    pub oauth_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<u64>,
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
        let content = serde_json::to_vec_pretty(self)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_path = path.with_extension(format!("json.tmp.{}-{timestamp}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path)?;
        if let Err(error) = file.write_all(&content).and_then(|()| file.sync_all()) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(error.into());
        }
        if let Err(error) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(error.into());
        }
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.cookies.is_some()
            || self.sapisid.is_some()
            || self.oauth_token.is_some()
            || self.refresh_token.is_some()
    }

    pub fn is_token_expired(&self) -> bool {
        if let Some(exp) = self.token_expiry {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now + 60 >= exp
        } else {
            self.oauth_token.is_none() && self.refresh_token.is_some()
        }
    }

    pub fn has_oauth(&self) -> bool {
        self.refresh_token.is_some() || self.oauth_token.is_some()
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
        assert_eq!(
            extract_sapisid_from_cookies(cookies),
            Some("abc_xyz_456".to_string())
        );
    }

    #[test]
    fn auth_file_is_written_with_private_permissions() {
        let root = std::env::temp_dir().join(format!(
            "ytm-rs-auth-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let path = root.join("auth.json");
        AuthConfig {
            cookies: Some("test-secret".to_string()),
            ..AuthConfig::default()
        }
        .save(&path)
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
