use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

use crate::{AppError, platform};

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub language: String,
    pub port: u16,
    pub lan_enabled: bool,
    pub token: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConfig {
    version: u8,
    language: String,
    port: u16,
    lan_enabled: bool,
    protected_token: Vec<u8>,
}

impl Config {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            language: "zh-CN".into(),
            port: 8001,
            lan_enabled: true,
            token: generate_token()?,
        })
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if !matches!(self.language.as_str(), "zh-CN" | "en") {
            return Err(AppError::new("invalid_language", ""));
        }
        if self.port == 0 {
            return Err(AppError::new("invalid_port", ""));
        }
        if self.token.len() < 32
            || self.token.len() > 256
            || !self.token.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(AppError::new("invalid_token", ""));
        }
        Ok(())
    }

    pub fn host(&self) -> &'static str {
        if self.lan_enabled {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }

    pub fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
}

pub fn generate_token() -> Result<String, AppError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| AppError::new("random_failed", e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn load(directory: &Path) -> Result<Config, AppError> {
    let path = directory.join("manager.json");
    if !path.exists() {
        let config = Config::new()?;
        save(directory, &config)?;
        return Ok(config);
    }
    let stored: StoredConfig = serde_json::from_slice(&fs::read(&path).map_err(AppError::storage)?)
        .map_err(AppError::storage)?;
    if stored.version != 1 {
        return Err(AppError::new("config_version", ""));
    }
    let config = Config {
        language: stored.language,
        port: stored.port,
        lan_enabled: stored.lan_enabled,
        token: String::from_utf8(platform::unprotect(&stored.protected_token)?)
            .map_err(AppError::storage)?,
    };
    config.validate()?;
    Ok(config)
}

pub fn save(directory: &Path, config: &Config) -> Result<(), AppError> {
    config.validate()?;
    fs::create_dir_all(directory).map_err(AppError::storage)?;
    let stored = StoredConfig {
        version: 1,
        language: config.language.clone(),
        port: config.port,
        lan_enabled: config.lan_enabled,
        protected_token: platform::protect(config.token.as_bytes())?,
    };
    let bytes = serde_json::to_vec_pretty(&stored).map_err(AppError::storage)?;
    platform::atomic_write(&directory.join("manager.json"), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let mut config = Config::new().unwrap();
        assert_eq!(config.language, "zh-CN");
        assert!(config.lan_enabled);
        assert_eq!(config.host(), "0.0.0.0");
        assert!(config.validate().is_ok());
        config.lan_enabled = false;
        assert_eq!(config.host(), "127.0.0.1");
        config.token = "a secret with spaces".into();
        assert_eq!(config.validate().unwrap_err().code, "invalid_token");
        config.token = "x".repeat(32);
        config.port = 0;
        assert_eq!(config.validate().unwrap_err().code, "invalid_port");
    }

    #[test]
    fn encrypted_config_roundtrip_and_replacement() {
        let dir =
            std::env::temp_dir().join(format!("windows-mcp-config-{}", generate_token().unwrap()));
        let mut config = load(&dir).unwrap();
        let disk = fs::read_to_string(dir.join("manager.json")).unwrap();
        assert!(!disk.contains(&config.token));
        config.language = "en".into();
        config.lan_enabled = false;
        save(&dir, &config).unwrap();
        assert_eq!(load(&dir).unwrap(), config);
        fs::remove_file(dir.join("manager.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn damaged_config_is_not_overwritten() {
        let dir =
            std::env::temp_dir().join(format!("windows-mcp-bad-{}", generate_token().unwrap()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manager.json"), "broken").unwrap();
        assert!(load(&dir).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("manager.json")).unwrap(),
            "broken"
        );
        fs::remove_file(dir.join("manager.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
