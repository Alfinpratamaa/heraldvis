//! heraldvis-core — tipe bersama, schema tool-call, error types.
//! Sesuai PRD §7 FR-1, §9, §13.6 (logging & schema validation).

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum CoreError {
    Validation(String),
    Serde(String),
    Io(String),
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(m) => write!(f, "validation error: {m}"),
            Self::Serde(m) => write!(f, "serde error: {m}"),
            Self::Io(m) => write!(f, "io error: {m}"),
        }
    }
}

impl std::error::Error for CoreError {}

/// Nama tool sesuai dataset training Qwen3.8 (PRD §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    OpenApplication,
    ReadFile,
    WriteFile,
    RunTest,
    GitOperation,
    ExecuteCommand,
    NavigateBrowser,
    OpenBrowser,
    PressKey,
    TypeText,
    TakeScreenshot,
    InspectScreen,
}

/// Wrapper native Qwen3.8 chat template:
/// `<tool_call><function=NAME><parameter=...>` → JSON yang setara.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: ToolName,
    /// Raw JSON params — divalidasi per-tool sebelum eksekusi.
    pub arguments: serde_json::Value,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    pub name: ToolName,
    pub id: Option<String>,
    pub result: ToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolResult {
    Success { output: String },
    Error { error: String },
}



#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApplicationParams {
    pub application: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileParams {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileParams {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunTestParams {
    pub command: String,
    #[serde(default)]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitOperationParams {
    pub command: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteCommandParams {
    pub command: String,
    #[serde(default)]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigateBrowserParams {
    pub url: String,
    #[serde(default)]
    pub browser: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PressKeyParams {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeTextParams {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeScreenshotParams {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectScreenParams {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub detail_level: Option<String>,
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::OpenApplication => "open_application",
            Self::ReadFile => "read_file",
            Self::WriteFile => "write_file",
            Self::RunTest => "run_test",
            Self::GitOperation => "git_operation",
            Self::ExecuteCommand => "execute_command",
            Self::NavigateBrowser => "navigate_browser",
            Self::OpenBrowser => "open_browser",
            Self::PressKey => "press_key",
            Self::TypeText => "type_text",
            Self::TakeScreenshot => "take_screenshot",
            Self::InspectScreen => "inspect_screen",
        };
        write!(f, "{s}")
    }
}

impl ToolCall {
    /// Parse dari JSON string (hasil extract dari `<tool_call>` block).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serde`] jika `s` bukan JSON `ToolCall` yang valid.
    pub fn from_json(s: &str) -> Result<Self, CoreError> {
        serde_json::from_str(s).map_err(|e| CoreError::Serde(e.to_string()))
    }

    pub fn to_response_success(&self, output: impl Into<String>) -> ToolResponse {
        ToolResponse {
            name: self.name.clone(),
            id: self.id.clone(),
            result: ToolResult::Success {
                output: output.into(),
            },
        }
    }

    pub fn to_response_error(&self, error: impl Into<String>) -> ToolResponse {
        ToolResponse {
            name: self.name.clone(),
            id: self.id.clone(),
            result: ToolResult::Error {
                error: error.into(),
            },
        }
    }

    /// Validasi schema per-tool (FR-1: schema validation ketat).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Validation`] jika argumen tidak sesuai schema per-tool.
    pub fn validate(&self) -> Result<(), CoreError> {
        match self.name {
            ToolName::OpenApplication => {
                let p: OpenApplicationParams = serde_json::from_value(self.arguments.clone())
                    .map_err(|e| CoreError::Validation(e.to_string()))?;
                if p.application.trim().is_empty() {
                    return Err(CoreError::Validation("application is empty".into()));
                }
            }
            ToolName::ReadFile | ToolName::WriteFile => {
                // path validation delegasi ke dispatcher whitelist
                let v: serde_json::Value = self.arguments.clone();
                if v.get("path").is_none() {
                    return Err(CoreError::Validation("missing field `path`".into()));
                }
            }
            ToolName::ExecuteCommand | ToolName::RunTest | ToolName::GitOperation => {
                let v: serde_json::Value = self.arguments.clone();
                if v.get("command").is_none() {
                    return Err(CoreError::Validation("missing field `command`".into()));
                }
            }
            ToolName::NavigateBrowser | ToolName::OpenBrowser => {
                let v: serde_json::Value = self.arguments.clone();
                if v.get("url").is_none() {
                    return Err(CoreError::Validation("missing field `url`".into()));
                }
                let url = v.get("url").and_then(|x| x.as_str()).unwrap_or("");
                if url.trim().is_empty() {
                    return Err(CoreError::Validation("url is empty".into()));
                }
                let lu = url.trim().to_ascii_lowercase();
                if !(lu.starts_with("http://") || lu.starts_with("https://") || lu.starts_with("file://")) {
                    return Err(CoreError::Validation("url must start with http://, https:// or file://".into()));
                }
            }
            ToolName::PressKey => {
                let p: PressKeyParams = serde_json::from_value(self.arguments.clone())
                    .map_err(|e| CoreError::Validation(e.to_string()))?;
                if p.key.trim().is_empty() {
                    return Err(CoreError::Validation("key is empty".into()));
                }
                if p.key.len() > 32 {
                    return Err(CoreError::Validation("key too long (max 32)".into()));
                }
            }
            ToolName::TypeText => {
                let p: TypeTextParams = serde_json::from_value(self.arguments.clone())
                    .map_err(|e| CoreError::Validation(e.to_string()))?;
                if p.text.is_empty() {
                    return Err(CoreError::Validation("text is empty".into()));
                }
                if p.text.len() > 4096 {
                    return Err(CoreError::Validation("text too long (max 4096)".into()));
                }
            }
            ToolName::TakeScreenshot => {
                let p: TakeScreenshotParams = serde_json::from_value(self.arguments.clone())
                    .map_err(|e| CoreError::Validation(e.to_string()))?;
                if p.path.trim().is_empty() {
                    return Err(CoreError::Validation("path is empty".into()));
                }
                if !p.path.to_ascii_lowercase().ends_with(".png") {
                    return Err(CoreError::Validation("path must end with .png".into()));
                }
            }
            ToolName::InspectScreen => {
                let p: InspectScreenParams = serde_json::from_value(self.arguments.clone())
                    .map_err(|e| CoreError::Validation(e.to_string()))?;
                if let Some(dl) = p.detail_level {
                    let d = dl.trim().to_ascii_lowercase();
                    if !d.is_empty() && d != "low" && d != "high" && d != "auto" {
                        return Err(CoreError::Validation(
                            "detail_level must be low, high or auto".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLogEntry {
    pub timestamp_ms: u64,
    pub tool_call: Option<ToolCall>,
    pub response: Option<ToolResponse>,
    pub blocked_by_whitelist: bool,
}

impl SessionLogEntry {
    #[must_use]
    pub fn new(tool_call: ToolCall) -> Self {
        Self {
            timestamp_ms: 0, // diisi dispatcher/net saat eksekusi
            tool_call: Some(tool_call),
            response: None,
            blocked_by_whitelist: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_roundtrip() {
        let tc = ToolCall {
            name: ToolName::ReadFile,
            arguments: serde_json::json!({"path": "/tmp/x.txt"}),
            id: Some("call_1".into()),
        };
        tc.validate().unwrap();
        let s = serde_json::to_string(&tc).unwrap();
        let back = ToolCall::from_json(&s).unwrap();
        assert_eq!(back.name, ToolName::ReadFile);
    }

    #[test]
    fn validation_rejects_empty() {
        let tc = ToolCall {
            name: ToolName::OpenApplication,
            arguments: serde_json::json!({"application": ""}),
            id: None,
        };
        assert!(tc.validate().is_err());
    }
}
