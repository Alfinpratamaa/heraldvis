//! heraldvis-dispatcher — tool dispatcher (PRD FR-1, FR-1a, M1 text-only).
//! Mengeksekusi 7 tool dengan schema validation + `whitelist`/sandboxing.
//! Full-auto tanpa approval gate — keamanan bergantung FR-1 `whitelist`.

use heraldvis_config::AppConfig;
use heraldvis_core::{CoreError, ToolCall, ToolName, ToolResponse};
use tracing::{info, warn};

pub use heraldvis_core::{ToolCall as CoreToolCall, ToolResponse as CoreToolResponse};

#[derive(Debug)]
pub enum DispatchError {
    BlockedByWhitelist(String),
    Validation(String),
    Execution(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockedByWhitelist(m) => write!(f, "blocked by whitelist: {m}"),
            Self::Validation(m) => write!(f, "validation: {m}"),
            Self::Execution(m) => write!(f, "execution: {m}"),
        }
    }
}
impl std::error::Error for DispatchError {}

impl From<CoreError> for DispatchError {
    fn from(e: CoreError) -> Self {
        Self::Validation(e.to_string())
    }
}

/// Dispatcher utama — stateless, hold reference ke `AppConfig`.
pub struct Dispatcher<'a> {
    config: &'a AppConfig,
}

impl<'a> Dispatcher<'a> {
    #[must_use]
    pub fn new(config: &'a AppConfig) -> Self {
        Self { config }
    }

    /// Entry point M1: validasi → whitelist check → eksekusi → `ToolResponse`.
    pub async fn dispatch(&self, call: &ToolCall) -> ToolResponse {
        if let Err(e) = call.validate() {
            warn!(tool = ?call.name, error = %e, "schema validation failed");
            return call.to_response_error(format!("schema validation: {e}"));
        }

        if let Err(blocked) = self.check_whitelist(call) {
            warn!(tool = ?call.name, reason = %blocked, "blocked by whitelist");
            return call.to_response_error(format!("blocked by whitelist: {blocked}"));
        }

        let result = match call.name {
            ToolName::OpenApplication => Self::handle_open_application(call),
            ToolName::ReadFile => self.handle_read_file(call).await,
            ToolName::WriteFile => self.handle_write_file(call).await,
            ToolName::RunTest => self.handle_run_test(call).await,
            ToolName::GitOperation => self.handle_git_operation(call).await,
            ToolName::ExecuteCommand => self.handle_execute_command(call).await,
            ToolName::NavigateBrowser | ToolName::OpenBrowser => Ok(Self::handle_navigate_browser(call)),
        };

        match result {
            Ok(output) => {
                info!(tool = ?call.name, "tool executed successfully");
                call.to_response_success(output)
            }
            Err(e) => {
                warn!(tool = ?call.name, error = %e, "tool execution failed");
                call.to_response_error(e.to_string())
            }
        }
    }

    fn check_whitelist(&self, call: &ToolCall) -> Result<(), String> {
        match call.name {
            ToolName::ReadFile | ToolName::WriteFile => {
                let path = call
                    .arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !self.config.is_path_allowed(path) {
                    return Err(format!("path not whitelisted: {path}"));
                }
            }
            ToolName::ExecuteCommand | ToolName::RunTest | ToolName::GitOperation => {
                let cmd = call
                    .arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !self.config.is_command_allowed(cmd) {
                    return Err(format!("command not whitelisted: {cmd}"));
                }
            }
            ToolName::OpenApplication
            | ToolName::NavigateBrowser
            | ToolName::OpenBrowser => {}
        }
        let enabled = match call.name {
            ToolName::OpenApplication => self.config.tools.open_application,
            ToolName::ReadFile => self.config.tools.read_file,
            ToolName::WriteFile => self.config.tools.write_file,
            ToolName::RunTest => self.config.tools.run_test,
            ToolName::GitOperation => self.config.tools.git_operation,
            ToolName::ExecuteCommand => self.config.tools.execute_command,
            ToolName::NavigateBrowser | ToolName::OpenBrowser => {
                self.config.tools.navigate_browser
            }
        };
        if !enabled {
            return Err(format!("tool {} disabled in config", call.name));
        }
        Ok(())
    }

    fn handle_open_application(call: &ToolCall) -> Result<String, DispatchError> {
        let app = call.arguments.get("application").and_then(|v| v.as_str()).unwrap_or("");
        let args: Vec<String> = call
            .arguments
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();

        // Headless WSL has no display; `enigo` synthetic input is not used here.
        let mut cmd = tokio::process::Command::new(app);
        cmd.args(&args);
        match cmd.spawn() {
            Ok(child) => {
                let _ = child.id();
                Ok(format!("launched application: {app} {}", args.join(" ")))
            }
            Err(e) => Err(DispatchError::Execution(format!("failed to launch {app}: {e}"))),
        }
    }

    /// # Errors
    /// Returns [`DispatchError::Execution`] jika file tidak dapat dibaca.
    async fn handle_read_file(&self, call: &ToolCall) -> Result<String, DispatchError> {
        let path = call.arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| DispatchError::Execution(format!("read_file {path}: {e}")))
    }

    /// # Errors
    /// Returns [`DispatchError::Execution`] jika parent dir atau write gagal.
    async fn handle_write_file(&self, call: &ToolCall) -> Result<String, DispatchError> {
        let path = call.arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let content = call.arguments.get("content").and_then(|v| v.as_str()).unwrap_or("");
        // Ensure parent dir exists to avoid silent write failure (FR reliability).
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                let parent_display = parent.display();
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| DispatchError::Execution(format!("mkdir {parent_display}: {e}")))?;
            }
        }
        tokio::fs::write(path, content)
            .await
            .map_err(|e| DispatchError::Execution(format!("write_file {path}: {e}")))?;
        Ok(format!("wrote {} bytes to {path}", content.len()))
    }

    async fn handle_run_test(&self, call: &ToolCall) -> Result<String, DispatchError> {
        self.run_command(call, "run_test").await
    }

    async fn handle_git_operation(&self, call: &ToolCall) -> Result<String, DispatchError> {
        self.run_command(call, "git_operation").await
    }

    async fn handle_execute_command(&self, call: &ToolCall) -> Result<String, DispatchError> {
        self.run_command(call, "execute_command").await
    }

    fn handle_navigate_browser(call: &ToolCall) -> String {
        let url = call.arguments.get("url").and_then(|v| v.as_str()).unwrap_or("");

        let opener = if cfg!(target_os = "linux") { "xdg-open" } else { "open" };
        match tokio::process::Command::new(opener).arg(url).spawn() {
            Ok(_child) => format!("opened browser at {url}"),
            Err(e) => {
                warn!("browser open failed ({opener}): {e}; url={url}");
                format!("browser open attempted at {url} (no display): {e}")
            }
        }
    }

    /// Jalankan `command` dari `ToolCall` via `sh -c`.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchError::Execution`] jika spawn gagal atau exit status non-zero.
    async fn run_command(&self, call: &ToolCall, label: &str) -> Result<String, DispatchError> {
        let command = call.arguments.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let workdir = call.arguments.get("workdir").and_then(|v| v.as_str());


        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        if let Some(dir) = workdir {
            cmd.current_dir(dir);
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| DispatchError::Execution(format!("{label} spawn failed: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = if stderr.is_empty() {
            stdout.to_string()
        } else {
            format!("{stdout}\n[stderr]\n{stderr}")
        };

        if output.status.success() {
            Ok(combined)
        } else {
            Err(DispatchError::Execution(format!(
                "{label} exited with {}: {combined}",
                output.status
            )))
        }
    }
}

// Keep optional linux crates linked even when not used in headless M1.
#[cfg(feature = "automation")]
#[allow(dead_code)]
fn _ensure_linux_deps_linked() {
    // enigo 0.3 — trait Keyboard/Mouse harus di-import agar trait object ter-resolve.
    use enigo::{Keyboard, Mouse};
    fn _enigo_type_check<T: Keyboard + Mouse>(_: &T) {}

    // zbus / atspi / notify — cukup referensi tipe agar cargo check mengkompilasi mereka.
    fn _zbus_check(_: Option<zbus::Connection>) {}
    fn _notify_check(_: Option<notify::RecommendedWatcher>) {}
}

#[cfg(not(feature = "automation"))]
#[allow(dead_code)]
fn _ensure_linux_deps_linked() {
    fn _notify_check(_: Option<notify::RecommendedWatcher>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use heraldvis_core::ToolCall;
    use serde_json::json;

    fn test_config() -> heraldvis_config::AppConfig {
        let mut cfg = heraldvis_config::AppConfig::default();
        cfg.whitelist.allowed_paths = vec!["/tmp/".into(), "/tmp/heraldvis/".into()];
        cfg
    }

    #[tokio::test]
    async fn blocked_by_path_whitelist() {
        let cfg = test_config();
        let d = Dispatcher::new(&cfg);
        let call = ToolCall {
            name: heraldvis_core::ToolName::ReadFile,
            arguments: json!({"path": "/etc/passwd"}),
            id: None,
        };
        let resp = d.dispatch(&call).await;
        match resp.result {
            heraldvis_core::ToolResult::Error { error } => assert!(error.contains("whitelist")),
            heraldvis_core::ToolResult::Success { .. } => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn write_and_read_roundtrip() {
        let mut cfg = test_config();
        cfg.whitelist.allowed_paths = vec!["/tmp/heraldvis-test-".into()];
        // tambah prefix yang cocok untuk /tmp/heraldvis-test-xxx
        cfg.whitelist.allowed_paths.push("/tmp/".into());
        let d = Dispatcher::new(&cfg);
        let path = "/tmp/heraldvis-test-dispatcher.txt";
        let write_call = ToolCall {
            name: heraldvis_core::ToolName::WriteFile,
            arguments: json!({"path": path, "content": "hello heraldvis"}),
            id: None,
        };
        let resp = d.dispatch(&write_call).await;
        assert!(matches!(resp.result, heraldvis_core::ToolResult::Success { .. }));

        let read_call = ToolCall {
            name: heraldvis_core::ToolName::ReadFile,
            arguments: json!({"path": path}),
            id: None,
        };
        let resp2 = d.dispatch(&read_call).await;
        match resp2.result {
            heraldvis_core::ToolResult::Success { output } => assert_eq!(output, "hello heraldvis"),
            heraldvis_core::ToolResult::Error { error } => panic!("read failed: {error}"),
        }
        let _ = tokio::fs::remove_file(path).await;
    }
}
