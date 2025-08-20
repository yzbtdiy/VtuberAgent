use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;

use crate::config::Settings;
use crate::models::{AuthData, AuthenticatedClient};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct AuthService {
    secret_key: String,
    valid_api_keys: Vec<String>,
    timestamp_tolerance: i64,
}

impl AuthService {
    pub fn new(settings: &Settings) -> Self {
        Self {
            secret_key: settings.auth.secret_key.clone(),
            valid_api_keys: settings.auth.valid_api_keys.clone(),
            timestamp_tolerance: settings.auth.timestamp_tolerance as i64,
        }
    }

    pub fn authenticate(&self, auth_data: &AuthData) -> Result<AuthenticatedClient> {
        match auth_data.auth_type.as_str() {
            "signature" => self.authenticate_signature(auth_data),
            "api_key" => self.authenticate_api_key(auth_data),
            _ => Err(anyhow!("Unsupported authentication type")),
        }
    }

    fn authenticate_signature(&self, auth_data: &AuthData) -> Result<AuthenticatedClient> {
        // Validate timestamp
        let now = Utc::now();
        let timestamp_diff = now.timestamp() - auth_data.timestamp.timestamp();
        if timestamp_diff.abs() > self.timestamp_tolerance {
            return Err(anyhow!("Timestamp is too old or too far in the future"));
        }

        // Validate API key
        if !self.valid_api_keys.contains(&auth_data.api_key) {
            return Err(anyhow!("Invalid API key"));
        }

        // Create signature data
        let signature_data = self.create_signature_data(auth_data)?;
        println!("Server signature data: {}", signature_data);

        // Verify signature
        let expected_signature = self.generate_signature(&signature_data)?;
        println!("Server expected signature: {}", expected_signature);
        println!("Client provided signature: {}", auth_data.signature);

        if auth_data.signature != expected_signature {
            return Err(anyhow!("Invalid signature"));
        }

        Ok(AuthenticatedClient {
            id: uuid::Uuid::new_v4(),
            user_id: auth_data.user_id.clone(),
            auth_type: "signature".to_string(),
            authenticated_at: now,
        })
    }

    fn authenticate_api_key(&self, auth_data: &AuthData) -> Result<AuthenticatedClient> {
        // Simple API key validation
        if !self.valid_api_keys.contains(&auth_data.api_key) {
            return Err(anyhow!("Invalid API key"));
        }

        Ok(AuthenticatedClient {
            id: uuid::Uuid::new_v4(),
            user_id: auth_data.user_id.clone(),
            auth_type: "api_key".to_string(),
            authenticated_at: Utc::now(),
        })
    }

    fn create_signature_data(&self, auth_data: &AuthData) -> Result<String> {
        let mut parts = HashMap::new();
        parts.insert("user_id", auth_data.user_id.as_str());
        parts.insert("api_key", auth_data.api_key.as_str());

        let timestamp_str = auth_data.timestamp.to_rfc3339();
        parts.insert("timestamp", &timestamp_str);
        parts.insert("nonce", auth_data.nonce.as_str());

        if let Some(user_data) = &auth_data.user_data {
            parts.insert("user_data", user_data);
        }

        // Sort keys for consistent signature generation
        let mut sorted_parts: Vec<_> = parts.into_iter().collect();
        sorted_parts.sort_by_key(|(k, _)| *k);

        let signature_string = sorted_parts
            .into_iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");

        Ok(signature_string)
    }

    fn generate_signature(&self, data: &str) -> Result<String> {
        let mut mac = HmacSha256::new_from_slice(self.secret_key.as_bytes())
            .map_err(|e| anyhow!("Invalid secret key: {}", e))?;

        mac.update(data.as_bytes());
        let result = mac.finalize();
        let signature = general_purpose::STANDARD.encode(result.into_bytes());

        Ok(signature)
    }

    #[allow(dead_code)]
    pub fn generate_client_signature(
        &self,
        user_id: &str,
        api_key: &str,
        timestamp: &DateTime<Utc>,
        nonce: &str,
        user_data: Option<&str>,
    ) -> Result<String> {
        let auth_data = AuthData {
            auth_type: "signature".to_string(),
            user_id: user_id.to_string(),
            api_key: api_key.to_string(),
            timestamp: *timestamp,
            nonce: nonce.to_string(),
            signature: String::new(), // Will be overwritten
            user_data: user_data.map(|s| s.to_string()),
        };

        let signature_data = self.create_signature_data(&auth_data)?;
        self.generate_signature(&signature_data)
    }
}
