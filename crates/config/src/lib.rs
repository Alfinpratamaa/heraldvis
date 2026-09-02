//! heraldvis-config — parsing config.toml (PRD FR-5, §14.3).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub mode: AppMode,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub whitelist: WhitelistConfig,
    #[serde(default)]
    pub voice: VoiceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    #[default]
    TextOnly,
    Voice,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default = "default_true")]
    pub open_application: bool,
    #[serde(default = "default_true")]
    pub read_file: bool,
    #[serde(default = "default_true")]
    pub write_file: bool,
    #[serde(default = "default_true")]
    pub run_test: bool,
    #[serde(default = "default_true")]
    pub git_operation: bool,
    #[serde(default = "default_true")]
    pub execute_command: bool,
    #[serde(default = "default_true")]
    pub navigate_browser: bool,
    #[serde(default = "default_true")]
    pub press_key: bool,
    #[serde(default = "default_true")]
    pub type_text: bool,
    #[serde(default = "default_true")]
    pub take_screenshot: bool,
    #[serde(default = "default_true")]
    pub inspect_screen: bool,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            open_application: true,
            read_file: true,
            write_file: true,
            run_test: true,
            git_operation: true,
            execute_command: true,
            navigate_browser: true,
            press_key: true,
            type_text: true,
            take_screenshot: true,
            inspect_screen: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistConfig {
    /// Jika false -> bypass semua whitelist (Full Access Mode).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Path prefix yang boleh diakses `read`/`write`.
    #[serde(default = "default_allowed_paths")]
    pub allowed_paths: Vec<String>,
    /// Command whitelist untuk `execute_command` (FR-1a safety).
    #[serde(default = "default_allowed_commands")]
    pub allowed_commands: Vec<String>,
}

impl Default for WhitelistConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_paths: default_allowed_paths(),
            allowed_commands: default_allowed_commands(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    #[serde(default = "default_stt_model")]
    pub stt_model: String,
    #[serde(default = "default_tts_model")]
    pub tts_model: String,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            stt_model: default_stt_model(),
            tts_model: default_tts_model(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            api_key: None,
            mode: AppMode::default(),
            tools: ToolsConfig::default(),
            whitelist: WhitelistConfig::default(),
            voice: VoiceConfig::default(),
        }
    }
}

fn default_endpoint() -> String {
    "http://127.0.0.1:8000".into()
}
fn default_true() -> bool {
    true
}
fn default_allowed_paths() -> Vec<String> {
    vec!["/tmp/heraldvis/".into(), "./workspace/".into()]
}
fn default_allowed_commands() -> Vec<String> {
    vec![
        "cargo".into(),
        "git".into(),
        "ls".into(),
        "cat".into(),
        "echo".into(),
        "npm".into(),
        "pytest".into(),
    ]
}
fn default_stt_model() -> String {
    "parakeet-tdt-1.1b".into()
}
fn default_tts_model() -> String {
    "kokoro-82m".into()
}

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "io error: {m}"),
            Self::Parse(m) => write!(f, "parse error: {m}"),
        }
    }
}
impl std::error::Error for ConfigError {}

/// Resolve koneksi dengan precedence FR-5a: CLI > env > config > default.
///
/// - `cli_endpoint`/`cli_api_key`: nilai dari flag `--endpoint`/`--api-key` (jika ada).
/// - Env: `HERALDVIS_ENDPOINT` / `HERALDVIS_API_KEY` (trimmed, non-empty).
/// - Fallback: `config.endpoint` → `http://127.0.0.1:8000` jika kosong.
#[must_use]
pub fn resolve_endpoint(
    cli_endpoint: Option<String>,
    config: &AppConfig,
) -> String {
    cli_endpoint
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("HERALDVIS_ENDPOINT")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| {
            let c = config.endpoint.trim().to_string();
            if c.is_empty() {
                default_endpoint()
            } else {
                c
            }
        })
}

/// Resolve `api_key` dengan precedence FR-5a: CLI > env > config (None jika kosong).
#[must_use]
pub fn resolve_api_key(
    cli_api_key: Option<String>,
    config: &AppConfig,
) -> Option<String> {
    cli_api_key
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("HERALDVIS_API_KEY")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            config
                .api_key
                .clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

impl AppConfig {
    /// Parse config dari string TOML.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] jika TOML tidak valid atau tidak sesuai schema.
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        toml::from_str(s).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Load config dari file TOML.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] jika file tidak dapat dibaca atau
    /// [`ConfigError::Parse`] jika isinya bukan TOML valid.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let s = std::fs::read_to_string(path.as_ref()).map_err(|e| ConfigError::Io(e.to_string()))?;
        Self::from_toml_str(&s)
    }

    /// Check apakah path lolos whitelist (prefix match, FR-1). Bypass jika enabled=false (Full Access).
    #[must_use]
    pub fn is_path_allowed(&self, path: &str) -> bool {
        if !self.whitelist.enabled {
            return true;
        }
        if self.whitelist.allowed_paths.is_empty() {
            return false;
        }
        self.whitelist
            .allowed_paths
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }

    /// Check apakah command lolos whitelist (FR-1a safety). Bypass jika enabled=false.
    #[must_use]
    pub fn is_command_allowed(&self, command: &str) -> bool {
        if !self.whitelist.enabled {
            return true;
        }
        let base = command.split_whitespace().next().unwrap_or("");
        self.whitelist.allowed_commands.iter().any(|c| c == base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrip() {
        let cfg = AppConfig::default();
        let s = toml::to_string(&cfg).unwrap();
        let back = AppConfig::from_toml_str(&s).unwrap();
        assert_eq!(back.endpoint, cfg.endpoint);
    }

    #[test]
    fn whitelist_checks() {
        let cfg = AppConfig::default();
        assert!(cfg.is_path_allowed("/tmp/heraldvis/foo.txt"));
        assert!(!cfg.is_path_allowed("/etc/passwd"));
        assert!(cfg.is_command_allowed("cargo test"));
        assert!(!cfg.is_command_allowed("rm -rf /"));
    }

    #[test]
    fn resolve_precedence_cli_over_env_over_config() {
        let mut cfg = AppConfig::default();
        cfg.endpoint = "http://config:8000".into();
        cfg.api_key = Some("cfg-key".into());
        // env should be overridden by cli
        std::env::set_var("HERALDVIS_ENDPOINT", "http://env:8000");
        std::env::set_var("HERALDVIS_API_KEY", "env-key");
        let ep = resolve_endpoint(Some("http://cli:8000".into()), &cfg);
        assert_eq!(ep, "http://cli:8000");
        let key = resolve_api_key(Some("cli-key".into()), &cfg);
        assert_eq!(key.as_deref(), Some("cli-key"));
        // env wins when cli None
        let ep2 = resolve_endpoint(None, &cfg);
        assert_eq!(ep2, "http://env:8000");
        let key2 = resolve_api_key(None, &cfg);
        assert_eq!(key2.as_deref(), Some("env-key"));
        std::env::remove_var("HERALDVIS_ENDPOINT");
        std::env::remove_var("HERALDVIS_API_KEY");
        // config wins when env absent
        let ep3 = resolve_endpoint(None, &cfg);
        assert_eq!(ep3, "http://config:8000");
        let key3 = resolve_api_key(None, &cfg);
        assert_eq!(key3.as_deref(), Some("cfg-key"));
        // fallback default when all empty
        let mut empty = AppConfig::default();
        empty.endpoint = String::new();
        empty.api_key = None;
        let ep4 = resolve_endpoint(None, &empty);
        assert_eq!(ep4, "http://127.0.0.1:8000");
        let key4 = resolve_api_key(None, &empty);
        assert!(key4.is_none());
    }
}
