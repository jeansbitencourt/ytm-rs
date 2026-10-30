use anyhow::Result;
use super::auth::AuthConfig;
use super::Track;

#[derive(Clone)]
pub struct InnertubeClient {
    pub auth: AuthConfig,
}

impl InnertubeClient {
    pub fn new(auth: AuthConfig) -> Self {
        Self { auth }
    }
}
