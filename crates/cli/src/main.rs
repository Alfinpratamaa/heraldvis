//! heraldvis — entrypoint binary (PRD §9 crates/cli, FR-1..FR-5, M5 full-duplex).
//!
//! M5 wiring: headless REPL now bridges `net` SSE/WebSocket + `voice` pipeline +
//! `dispatcher` auto-dispatch + FR-4 session logging + sentence-split TTS queue
//! + barge-in + WS reconnect with text fallback (PRD §12).

#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc, clippy::doc_markdown, clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::too_many_lines, clippy::needless_pass_by_value)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use heraldvis_config::AppConfig;
use heraldvis_core::{SessionLogEntry, ToolCall, ToolResult};
use heraldvis_dispatcher::Dispatcher;
use heraldvis_net::{HeraldvisClient, NetConfig, SseEvent};
use heraldvis_voice::{VoiceConfig, VoicePipeline, VoiceStatus};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Helpers: config mapping + session logger (FR-4, §13.6)
// ---------------------------------------------------------------------------

fn net_config_from_app(cfg: &AppConfig) -> NetConfig {
    NetConfig {
        endpoint: cfg.endpoint.clone(),
        api_key: cfg.api_key.clone(),
        ..Default::default()
    }
}

fn voice_config_from_app(cfg: &AppConfig) -> VoiceConfig {
    // AppConfig.voice currently holds stt/tts model names; map to VoiceConfig defaults.
    // VAD model path can be overridden via env HERALDVIS_VAD_MODEL for headless tests.
    let vad_path = std::env::var("HERALDVIS_VAD_MODEL")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.exists());
    let _ = &cfg.voice; // keep dependency, models logged below
    VoiceConfig {
        vad_model_path: vad_path,
        ..Default::default()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Minimal FR-4 session logger: appends JSONL to /tmp/heraldvis/sessions/… + in-mem buffer.
struct SessionLogger {
    path: PathBuf,
    entries: Vec<SessionLogEntry>,
}

impl SessionLogger {
    fn new() -> Self {
        let ts = now_ms();
        let dir = PathBuf::from("/tmp/heraldvis/sessions");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("session-{ts}.jsonl"));
        info!(path=%path.display(), "session log opened");
        Self {
            path,
            entries: Vec::new(),
        }
    }

    fn log(&mut self, tool_call: Option<ToolCall>, response: Option<heraldvis_core::ToolResponse>, blocked: bool) {
        let mut entry = if let Some(tc) = tool_call.clone() {
            SessionLogEntry::new(tc)
        } else {
            SessionLogEntry {
                timestamp_ms: 0,
                tool_call: None,
                response: None,
                blocked_by_whitelist: blocked,
            }
        };
        entry.timestamp_ms = now_ms();
        entry.response = response;
        entry.blocked_by_whitelist = blocked;
        // append JSONL best-effort
        if let Ok(line) = serde_json::to_string(&entry) {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "{line}")
                });
        }
        self.entries.push(entry);
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// OpenAI tools schema (mirrors core ToolName 7 tools)
// ---------------------------------------------------------------------------

fn openai_tools_schema() -> serde_json::Value {
    serde_json::json!([
        {"type":"function","function":{"name":"open_application","description":"Open desktop application","parameters":{"type":"object","properties":{"application":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["application"]}}},
        {"type":"function","function":{"name":"read_file","description":"Read file content","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
        {"type":"function","function":{"name":"write_file","description":"Write file","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}},
        {"type":"function","function":{"name":"run_test","description":"Run test command","parameters":{"type":"object","properties":{"command":{"type":"string"},"workdir":{"type":"string"}},"required":["command"]}}},
        {"type":"function","function":{"name":"git_operation","description":"Git operation","parameters":{"type":"object","properties":{"command":{"type":"string"},"workdir":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["command"]}}},
        {"type":"function","function":{"name":"execute_command","description":"Execute shell command (whitelisted)","parameters":{"type":"object","properties":{"command":{"type":"string"},"workdir":{"type":"string"}},"required":["command"]}}},
        {"type":"function","function":{"name":"navigate_browser","description":"Open browser URL","parameters":{"type":"object","properties":{"url":{"type":"string"},"browser":{"type":"string"}},"required":["url"]}}}
    ])
}

fn build_chat_payload(user_msg: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "qwen3.8",
        "messages": [{"role":"user","content": user_msg}],
        "stream": true,
        "tools": openai_tools_schema(),
        "tool_choice": "auto"
    })
}

// Accumulator for streaming tool_calls deltas per index
#[derive(Debug, Default)]
#[allow(dead_code)]
struct ToolCallAccum {
    id: Option<String>,
    name: Option<String>,
    args_buf: String,
    index: u32,
}

fn placeholder_pcm_for_sentence(sentence: &str) -> Vec<f32> {
    // 0.15s @16k = 2400 samples, amplitude scaled by chars len to make queue observable in tests
    let n = 2400usize;
    let amp = 0.15f32;
    let freq = 220.0 + (sentence.len() as f32 % 200.0);
    (0..n)
        .map(|i| {
            let t = i as f32 / 16_000.0;
            amp * (2.0 * std::f32::consts::PI * freq * t).sin()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Chat streaming + auto-dispatch (M5 core)
// ---------------------------------------------------------------------------

async fn run_chat_turn(
    client: &HeraldvisClient,
    dispatcher: &Dispatcher<'_>,
    pipeline: &mut VoicePipeline,
    logger: &mut SessionLogger,
    user_msg: &str,
) -> anyhow::Result<()> {
    let payload = build_chat_payload(user_msg);
    let started = now_ms();
    info!(msg=%user_msg, "chat_stream → VPS");

    let mut stream = match client.chat_stream(payload).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error=%e, "VPS chat_stream failed — fallback text (PRD §12), staying in REPL");
            println!("[offline fallback] VPS unreachable ({e}). Try direct ToolCall JSON or check endpoint {}.", client.config().endpoint);
            return Ok(());
        }
    };

    let mut text_buf = String::new();
    let mut sentence_buf = String::new();
    let mut accums: HashMap<u32, ToolCallAccum> = HashMap::new();
    let mut finish_reason: Option<String> = None;

    // Mark thinking
    if pipeline.status() == VoiceStatus::Listening || pipeline.status() == VoiceStatus::Idle {
        // pipeline has no set_status; simulate via enqueue? we keep status Idle→Thinking via log
        info!("LLM thinking (streaming)");
    }

    while let Some(ev) = stream.next().await {
        match ev {
            Ok(SseEvent::Chunk(chunk)) => {
                for choice in chunk.choices {
                    if let Some(fr) = choice.finish_reason {
                        finish_reason = Some(fr);
                    }
                    if let Some(content) = choice.delta.content {
                        text_buf.push_str(&content);
                        sentence_buf.push_str(&content);
                        print!("{content}");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        // per-sentence TTS queue (PRD §14.2 split, §14.4 time-to-first-audio <300ms)
                        let sentences = VoicePipeline::split_sentences(&sentence_buf);
                        if sentences.len() > 1 || (finish_reason.is_some() && !sentence_buf.trim().is_empty()) {
                            // keep last as carry if not finished, otherwise flush all
                            let (to_enqueue, carry) = if finish_reason.is_some() {
                                (sentences.clone(), String::new())
                            } else {
                                let n = sentences.len() - 1;
                                (sentences[..n].to_vec(), sentences[n].clone())
                            };
                            for sentence in &to_enqueue {
                                let pcm = placeholder_pcm_for_sentence(sentence);
                                let before = pipeline.playback_len();
                                pipeline.enqueue_pcm(pcm);
                                info!(sentence=%sentence, before, after=%pipeline.playback_len(), "TTS sentence enqueued");
                            }
                            sentence_buf = carry;
                        }
                    }
                    if let Some(tcs) = choice.delta.tool_calls {
                        for tc in tcs {
                            let idx = tc.index.unwrap_or(0);
                            let entry = accums.entry(idx).or_insert_with(|| ToolCallAccum {
                                index: idx,
                                ..Default::default()
                            });
                            if let Some(id) = tc.id {
                                entry.id = Some(id);
                            }
                            if let Some(f) = tc.function {
                                if let Some(name) = f.name {
                                    entry.name = Some(name);
                                }
                                if let Some(args) = f.arguments {
                                    entry.args_buf.push_str(&args);
                                }
                            }
                        }
                    }
                }
            }
            Ok(SseEvent::Done) => break,
            Ok(SseEvent::Comment(_)) => {}
            Err(e) => {
                warn!(error=%e, "SSE parse error");
                break;
            }
        }
        // barge-in check: if pipeline Speaking and mock VAD detects speech, interrupt
        // In text mode we simulate by checking if pipeline playback grew and user typed? no-op.
        // Real voice mode feeds mic frames via process_resampled_frames in WS task.
    }
    println!();

    // flush remaining sentence
    if !sentence_buf.trim().is_empty() {
        for sentence in VoicePipeline::split_sentences(&sentence_buf) {
            let pcm = placeholder_pcm_for_sentence(&sentence);
            pipeline.enqueue_pcm(pcm);
        }
    }

    let latency = now_ms().saturating_sub(started);
    if !text_buf.trim().is_empty() {
        info!(latency_ms=%latency, chars=%text_buf.len(), "LLM text complete");
    }

    // Dispatch accumulated tool calls
    if !accums.is_empty() {
        info!(count=%accums.len(), "dispatching streamed tool_calls");
        for (_, accum) in accums {
            let name_str = accum.name.clone().unwrap_or_default();
            let tool_name = match name_str.as_str() {
                "open_application" => heraldvis_core::ToolName::OpenApplication,
                "read_file" => heraldvis_core::ToolName::ReadFile,
                "write_file" => heraldvis_core::ToolName::WriteFile,
                "run_test" => heraldvis_core::ToolName::RunTest,
                "git_operation" => heraldvis_core::ToolName::GitOperation,
                "execute_command" => heraldvis_core::ToolName::ExecuteCommand,
                "navigate_browser" | "open_browser" => heraldvis_core::ToolName::NavigateBrowser,
                other => {
                    warn!(tool=%other, "unknown tool from model, skipping");
                    continue;
                }
            };
            let args: serde_json::Value = if accum.args_buf.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&accum.args_buf).unwrap_or_else(|e| {
                    warn!(args=%accum.args_buf, error=%e, "tool args JSON parse failed, using empty");
                    serde_json::json!({})
                })
            };
            let call = ToolCall {
                name: tool_name.clone(),
                arguments: args,
                id: accum.id.clone(),
            };
            let t0 = now_ms();
            let resp = dispatcher.dispatch(&call).await;
            let is_blocked = matches!(resp.result, ToolResult::Error { ref error } if error.contains("whitelist") || error.contains("blocked"));
            let dt = now_ms().saturating_sub(t0);
            info!(tool=%tool_name, latency_ms=%dt, blocked=%is_blocked, "tool_response");
            println!("{}", serde_json::to_string(&resp).unwrap_or_default());
            // FR-4 log
            logger.log(Some(call), Some(resp), is_blocked);
        }
    } else if text_buf.trim().is_empty() && finish_reason.is_none() {
        warn!("empty LLM response (no content, no tool_calls)");
    }

    // Simulate playback drain in headless (real cpal drains via callback)
    if pipeline.playback_len() > 0 {
        let drained = pipeline.drain_playback(8000);
        info!(drained=%drained.len(), remaining=%pipeline.playback_len(), "playback drained (headless mock)");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "--gui") {
        return run_gui().await;
    }
    if args.iter().any(|a| a == "--check") {
        return run_check().await;
    }
    run_headless().await
}

fn print_help() {
    println!(
        "\
heraldvis — local desktop voice interpreter (Qwen3.8)

Usage:
  heraldvis                          # headless REPL (text+voice, M5)
  heraldvis --check                  # dispatcher self-test (no VPS)
  heraldvis --gui                    # floating overlay (needs display+gui feature)
  heraldvis --voice                  # force voice mode (start capture)
  heraldvis --endpoint URL           # override VPS endpoint

REPL:
  - ToolCall JSON per line → dispatch: {{\"name\":\"write_file\",\"arguments\":{{\"path\":\"/tmp/heraldvis/hi.txt\",\"content\":\"hi\"}}}}
  - Plain text → LLM chat_stream → auto tool dispatch + sentence TTS queue
  - exit / quit to leave

Config: config.toml (or config.example.toml), env HERALDVIS_VAD_MODEL, --endpoint"
    );
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
    // M5 extra: voice pipeline + net URL build check (no network)
    let vp = VoicePipeline::new(voice_config_from_app(&cfg));
    let nc = net_config_from_app(&cfg);
    println!("CHECK OK — voice status {:?}, ws_url {}", vp.status(), nc.ws_url("/ws/audio"));
    println!("CHECK OK — sentence split: {:?}", VoicePipeline::split_sentences("Hello main.rs. How are you?"));
    Ok(())
}

async fn run_headless() -> anyhow::Result<()> {
    let mut cfg = load_config();
    // CLI overrides (PRD FR-5)
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--voice") {
        cfg.mode = heraldvis_config::AppMode::Voice;
    }
    for w in args.windows(2) {
        if w[0] == "--endpoint" {
            cfg.endpoint.clone_from(&w[1]);
        }
    }

    let net_cfg = net_config_from_app(&cfg);
    let client = HeraldvisClient::new(net_cfg.clone());
    let voice_cfg = voice_config_from_app(&cfg);
    let mut pipeline = VoicePipeline::new(voice_cfg);
    let dispatcher = Dispatcher::new(&cfg);
    let mut logger = SessionLogger::new();

    info!(endpoint=%cfg.endpoint, mode=?cfg.mode, stt=%cfg.voice.stt_model, tts=%cfg.voice.tts_model, "heraldvis M5 headless running");
    info!("Modes: ToolCall JSON per line OR plain chat text → VPS. `exit` to quit.");

    // Voice capture if mode Voice (M3/M4)
    if cfg.mode == heraldvis_config::AppMode::Voice {
        pipeline.start_capture();
        info!(status=?pipeline.status(), "voice capture on (barge-in armed)");
        // WS audio placeholder: try connect with reconnect, fallback to text (PRD §12)
        let ws_path = "/ws/audio";
        let ws_url = net_cfg.ws_url(ws_path);
        info!(url=%ws_url, "voice WS connect (with reconnect)");
        let client_clone = HeraldvisClient::new(net_cfg.clone());
        tokio::spawn(async move {
            match client_clone.connect_ws_with_reconnect(ws_path).await {
                Ok(mut conn) => {
                    info!("WS audio connected — placeholder loop (real audio forwarding would run here)");
                    // Minimal keepalive: ping every 15s, barge-in would send {"type":"interrupt"}
                    let _ = conn.send_text(serde_json::json!({"type":"hello","client":"heraldvis M5"}).to_string()).await;
                    // Hold open briefly then close for demo; real impl would stream mic PCM
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    let _ = conn.close().await;
                }
                Err(e) => {
                    warn!(error=%e, "WS audio unavailable — fallback text mode (PRD §12)");
                }
            }
        });
    } else {
        info!("text_only mode — voice pipeline idle. Use --voice to enable capture+VAD.");
    }

    // REPL
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    println!("heraldvis M5 — type ToolCall JSON or plain chat text. `exit` to quit.");
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }

        // Try ToolCall JSON first (existing M1 behavior preserved)
        if let Ok(call) = serde_json::from_str::<ToolCall>(&line) {
            let t0 = now_ms();
            let resp = dispatcher.dispatch(&call).await;
            let is_blocked = matches!(resp.result, ToolResult::Error { ref error } if error.contains("whitelist") || error.contains("blocked"));
            let dt = now_ms().saturating_sub(t0);
            info!(tool=?resp.name, latency_ms=%dt, blocked=%is_blocked, "tool_response direct");
            println!("{}", serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into()));
            logger.log(Some(call), Some(resp), is_blocked);
            continue;
        }

        // Also accept wrapped {"name":...,"arguments":...} without ToolCall validation? already covered.
        // Plain text → LLM streaming + auto-dispatch
        if let Err(e) = run_chat_turn(&client, &dispatcher, &mut pipeline, &mut logger, &line).await {
            warn!(error=%e, "chat turn failed");
            println!("[error] chat turn: {e}");
        }
        info!(session_entries=%logger.len(), playback=%pipeline.playback_len(), status=?pipeline.status(), "turn done");
    }

    pipeline.stop_capture();
    info!(entries=%logger.len(), status=?pipeline.status(), "heraldvis headless exit");
    Ok(())
}

#[cfg(feature = "gui")]
async fn run_gui() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 360.0])
            .with_transparent(true)
            .with_decorations(false)
            .with_always_on_top()
            .with_title("Heraldvis — Jarvis Overlay (M5)"),
        ..Default::default()
    };
    let mut cfg = load_config();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--voice") {
        cfg.mode = heraldvis_config::AppMode::Voice;
    }
    let app_name = format!("Heraldvis — {} ({:?}) M5", cfg.endpoint, cfg.mode);
    match eframe::run_native(
        &app_name,
        options,
        Box::new(|_cc| Ok(Box::new(HeraldvisApp::new(cfg)))),
    ) {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!(error=%e, "eframe failed (likely no display/Wayland); falling back to headless");
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
                    info!(path=%p, "loaded config");
                    return c;
                }
                Err(e) => warn!(path=%p, error=%e, "failed to parse config, using defaults"),
            }
        }
    }
    AppConfig::default()
}

// ---------------------------------------------------------------------------
// GUI (FR-1b) — M5 live status
// ---------------------------------------------------------------------------

#[cfg(feature = "gui")]
struct HeraldvisApp {
    cfg: AppConfig,
    pipeline: VoicePipeline,
    status: String,
    transcript: String,
    minimized: bool,
    chat_input: String,
}

#[cfg(feature = "gui")]
impl HeraldvisApp {
    fn new(cfg: AppConfig) -> Self {
        let pipeline = VoicePipeline::new(voice_config_from_app(&cfg));
        let mode_hint = match cfg.mode {
            heraldvis_config::AppMode::Voice => "voice capture armed (barge-in ready)",
            heraldvis_config::AppMode::TextOnly => "text_only — use --voice for capture",
        };
        Self {
            cfg,
            pipeline,
            status: "idle".into(),
            transcript: format!(
                "Heraldvis M5 — full-duplex ready.\nEndpoint: {{}}\nMode hint: {mode_hint}\nType chat or ToolCall JSON in headless; here use buttons.\n",
                String::new()
            ),
            minimized: false,
            chat_input: String::new(),
        }
    }

    fn status_color(&self) -> egui::Color32 {
        match self.pipeline.status() {
            VoiceStatus::Idle => egui::Color32::from_rgb(180, 180, 180),
            VoiceStatus::Listening => egui::Color32::from_rgb(120, 220, 255),
            VoiceStatus::Thinking => egui::Color32::from_rgb(255, 210, 90),
            VoiceStatus::Speaking => egui::Color32::from_rgb(180, 255, 180),
        }
    }
}

#[cfg(feature = "gui")]
impl eframe::App for HeraldvisApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Update status string from pipeline
        self.status = format!("{:?}", self.pipeline.status());

        egui::CentralPanel::default().show(ctx, |ui| {
            // Draggable header
            let header = ui.horizontal(|ui| {
                let resp = ui.label(
                    egui::RichText::new("◆ Heraldvis M5")
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
                            .color(self.status_color()),
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
            ui.label(format!(
                "Capture: {} | Playback: {} frames | VAD: mock (set HERALDVIS_VAD_MODEL for ort)",
                if self.pipeline.is_capturing() { "on" } else { "off" },
                self.pipeline.playback_len()
            ));
            ui.separator();
            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                ui.label(egui::RichText::new(&self.transcript).small().monospace());
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("chat:");
                let resp = ui.text_edit_singleline(&mut self.chat_input);
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let msg = self.chat_input.trim().to_string();
                    if !msg.is_empty() {
                        self.transcript.push_str(&format!("\n> {msg}\n"));
                        self.chat_input.clear();
                        // Mock streaming: split into sentences and enqueue pcm to show TTS queue
                        for s in VoicePipeline::split_sentences(&msg) {
                            let pcm = placeholder_pcm_for_sentence(&s);
                            self.pipeline.enqueue_pcm(pcm);
                        }
                        self.pipeline.start_capture();
                    }
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Start Listening").clicked() {
                    self.pipeline.start_capture();
                    self.transcript.push_str("\n[listening — mic open, VAD armed]\n");
                }
                if ui.button("Barge-in (test)").clicked() {
                    // Simulate loud frame while speaking
                    self.pipeline.enqueue_pcm(vec![0.9; 2048]);
                    let loud = vec![0.5f32; 512];
                    let r = self.pipeline.process_vad_frame(&loud);
                    self.transcript
                        .push_str(&format!("\n[barge-in prob={:.2} → {:?}]\n", r.prob, self.pipeline.status()));
                }
                if ui.button("Clear").clicked() {
                    self.transcript.clear();
                    self.pipeline.clear_playback_queue();
                    self.pipeline.stop_capture();
                }
            });
            ui.small("M5: headless does LLM stream→auto-dispatch+TTS+WS; GUI mirrors pipeline live.");
        });
    }
}
