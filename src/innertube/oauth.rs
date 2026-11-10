use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub const DEFAULT_CLIENT_ID: &str =
    "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com";
pub const DEFAULT_CLIENT_SECRET: &str = "SboVhoG9s0rNafixCSGGKXAT";
pub const OAUTH_SCOPE: &str = "http://gdata.youtube.com https://www.googleapis.com/auth/youtube";
pub const DEVICE_CODE_URL: &str = "https://oauth2.googleapis.com/device/code";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub token_type: String,
}

#[derive(Debug, Clone)]
pub enum PollResult {
    Success(TokenResponse),
    Pending,
    SlowDown,
    Expired,
    Error(String),
}

/// Requests a device authorization code from Google OAuth 2.0 (RFC 8628)
pub async fn request_device_code(client_id: &str) -> Result<DeviceCodeResponse> {
    let http = Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .unwrap_or_default();

    let params = [("client_id", client_id), ("scope", OAUTH_SCOPE)];

    let resp = http
        .post(DEVICE_CODE_URL)
        .form(&params)
        .send()
        .await
        .context("Failed to send request to Google Device Code endpoint")?;

    if !resp.status().is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Google Device Code request failed: {}", err_text);
    }

    let val: Value = resp
        .json()
        .await
        .context("Failed to parse device code JSON")?;

    let device_code = val
        .get("device_code")
        .and_then(|v| v.as_str())
        .context("Missing device_code")?
        .to_string();
    let user_code = val
        .get("user_code")
        .and_then(|v| v.as_str())
        .context("Missing user_code")?
        .to_string();
    let verification_url = val
        .get("verification_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://www.google.com/device")
        .to_string();
    let expires_in = val
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(1800);
    let interval = val.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);

    Ok(DeviceCodeResponse {
        device_code,
        user_code,
        verification_url,
        expires_in,
        interval,
    })
}

/// Polls Google OAuth token endpoint once
pub async fn poll_token_once(
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> Result<PollResult> {
    let http = Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .unwrap_or_default();

    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", device_code),
        ("grant_type", "http://oauth.net/grant_type/device/1.0"),
    ];

    let resp = http
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .context("Failed to send token poll request")?;

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();

    if status.is_success() {
        if let Ok(tok) = serde_json::from_str::<TokenResponse>(&body_text) {
            return Ok(PollResult::Success(tok));
        }
    }

    if let Ok(err_json) = serde_json::from_str::<Value>(&body_text) {
        if let Some(err_str) = err_json.get("error").and_then(|e| e.as_str()) {
            match err_str {
                "authorization_pending" => return Ok(PollResult::Pending),
                "slow_down" => return Ok(PollResult::SlowDown),
                "expired_token" => return Ok(PollResult::Expired),
                other => return Ok(PollResult::Error(other.to_string())),
            }
        }
    }

    Ok(PollResult::Error(body_text))
}

/// Refreshes an existing access token using the permanent refresh token
pub async fn refresh_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse> {
    let http = Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .unwrap_or_default();

    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    let resp = http
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .context("Failed to send token refresh request")?;

    if !resp.status().is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Google token refresh failed: {}", err_text);
    }

    let mut tok: TokenResponse = resp
        .json()
        .await
        .context("Failed to parse token refresh JSON")?;
    // Some providers do not re-emit the refresh token; retain the original one
    if tok.refresh_token.is_none() {
        tok.refresh_token = Some(refresh_token.to_string());
    }

    Ok(tok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poll_result_parsing() {
        let json_success = r#"{
            "access_token": "ya29.test_token_123",
            "expires_in": 3599,
            "refresh_token": "1//04test_refresh",
            "token_type": "Bearer"
        }"#;

        let tok: TokenResponse = serde_json::from_str(json_success).unwrap();
        assert_eq!(tok.access_token, "ya29.test_token_123");
        assert_eq!(tok.refresh_token.as_deref(), Some("1//04test_refresh"));
        assert_eq!(tok.expires_in, 3599);
    }

    #[test]
    fn test_device_code_response_structure() {
        let raw = r#"{
            "device_code": "dev123",
            "user_code": "ABCD-EFGH",
            "verification_url": "https://www.google.com/device",
            "expires_in": 1800,
            "interval": 5
        }"#;

        let res: DeviceCodeResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(res.user_code, "ABCD-EFGH");
        assert_eq!(res.verification_url, "https://www.google.com/device");
        assert_eq!(res.interval, 5);
    }
}
