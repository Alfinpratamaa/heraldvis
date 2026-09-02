//! heraldvis — entrypoint binary (PRD §9 crates/cli, FR-1b GUI overlay).

use heraldvis_config::AppConfig;
use heraldvis_core::{ToolCall, ToolResult};
use heraldvis_dispatcher::Dispatcher;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--gui") {
        return run_gui().await;
    }
    if args.iter().any(|a| a == "--check") {
        return run_check().await;
    }
    run_headless().await
}

async fn run_check() -> anyhow::Result<()> {
    let cfg = load_config();
    info!(endpoint = %cfg.endpoint, mode = ?cfg.mode, "config loaded (check mode)");
    let d = Dispatcher::new(&cfg);
    let dummy = ToolCall {
        name: heraldvis_core::ToolName::ExecuteCommand,
        arguments: serde_json::json!({"command": "echo heraldvis check"}),
        id: Some("check_1".into()),
    };
    let resp = d.dispatch(&dummy).await;
    match resp.result {
        ToolResult::Success { output } => {
            println!("CHECK OK — dispatcher works: {output}");
        }
        ToolResult::Error { error } => {
            eprintln!("CHECK FAILED: {error}");
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn run_headless() -> anyhow::Result<()> {
    let cfg = load_config();
    let dispatcher = Dispatcher::new(&cfg);
    info!("heraldvis headless running — pipe ToolCall JSON per line to stdin (Ctrl-D to exit)");
    info!(endpoint = %cfg.endpoint, "connected endpoint (M1: no WS yet)");
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }
        let call: Result<ToolCall, _> = serde_json::from_str(&line);
        let call = match call {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, input = %line, "invalid ToolCall JSON");
                println!(
                    "{}",
                    serde_json::json!({"status":"error","error": format!("invalid ToolCall JSON: {e}")})
                );
                continue;
            }
        };
        let resp = dispatcher.dispatch(&call).await;
        info!(tool = ?resp.name, result = ?resp.result, "tool_response");
        println!("{}", serde_json::to_string(&resp)?);
    }
    info!("heraldvis headless exit");
    Ok(())
}

#[cfg(feature = "gui")]
async fn run_gui() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 320.0])
            .with_transparent(true)
            .with_decorations(false)
            .with_always_on_top()
            .with_title("Heraldvis — Jarvis Overlay (M1)"),
        ..Default::default()
    };
    let cfg = load_config();
    let app_name = format!("Heraldvis — {} ({:?})", cfg.endpoint, cfg.mode);
    match eframe::run_native(
        &app_name,
        options,
        Box::new(|_cc| Ok(Box::new(HeraldvisApp::new(cfg)))),
    ) {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!(error = %e, "eframe failed (likely no display/Wayland); falling back to headless");
            println!("GUI not available (no display): {e}");
            Ok(())
        }
    }
}

#[cfg(not(feature = "gui"))]
async fn run_gui() -> anyhow::Result<()> {
    eprintln!(
        "Fitur GUI tidak di-build. Install deps apt (PRD §14.5) lalu build dengan:\n  cargo run -p heraldvis --features gui -- --gui\natau aktifkan default gui di Cargo.toml."
    );
    run_check().await
}

fn load_config() -> AppConfig {
    for p in ["config.toml", "config.example.toml"] {
        if std::path::Path::new(p).exists() {
            match AppConfig::from_file(p) {
                Ok(c) => {
                    info!(path = %p, "loaded config");
                    return c;
                }
                Err(e) => warn!(path = %p, error = %e, "failed to parse config, using defaults"),
            }
        }
    }
    AppConfig::default()
}

#[cfg(feature = "gui")]
struct HeraldvisApp {
    cfg: AppConfig,
    status: String,
    transcript: String,
    minimized: bool,
}

#[cfg(feature = "gui")]
impl HeraldvisApp {
    fn new(cfg: AppConfig) -> Self {
        Self {
            cfg,
            status: "idle".into(),
            transcript: "Heraldvis M1 — text-only dispatcher ready.\nPipe ToolCall JSON via headless mode, or use --check.\n".into(),
            minimized: false,
        }
    }
}

#[cfg(feature = "gui")]
impl eframe::App for HeraldvisApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let header = ui.horizontal(|ui| {
                let resp = ui.label(
                    egui::RichText::new("◆ Heraldvis")
                        .strong()
                        .color(egui::Color32::from_rgb(120, 220, 255)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(if self.minimized { "□" } else { "—" })
                        .clicked()
                    {
                        self.minimized = !self.minimized;
                    }
                    ui.label(
                        egui::RichText::new(&self.status)
                            .small()
                            .color(egui::Color32::from_rgb(180, 255, 180)),
                    );
                });
                resp.response
            });
            if header.response.interact(egui::Sense::drag()).dragged() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.separator();
            if self.minimized {
                ui.label(egui::RichText::new("minimized — click □ to expand").weak().small());
                return;
            }
            ui.label(format!("Endpoint: {} ({:?})", self.cfg.endpoint, self.cfg.mode));
            ui.separator();
            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                ui.label(egui::RichText::new(&self.transcript).small().monospace());
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Check dispatcher (--check)").clicked() {
                    self.status = "checking...".into();
                    self.transcript.push_str("\n[check requested — run `heraldvis --check` in terminal]\n");
                }
                if ui.button("Clear").clicked() {
                    self.transcript.clear();
                }
            });
            ui.small("M1 skeleton — voice/WS full in M2/M3");
        });
    }
}
