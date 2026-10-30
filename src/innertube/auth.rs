use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub cookies: Option<String>,
    pub sapisid: Option<String>,
    pub authorization: Option<String>,
    pub oauth_token: Option<String>,
}

impl AuthConfig {
    pub fn is_authenticated(&self) -> bool {
        self.cookies.is_some() || self.sapisid.is_some() || self.oauth_token.is_some()
    }
}
