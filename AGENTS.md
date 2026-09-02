# AGENTS.md — Heraldvis

> Local desktop voice interpreter (Rust) for Qwen3.8 tool-calling + full-duplex voice. Single-user Ubuntu client; model runs on VPS (vLLM/SGLang). See `PRD.md` and `README.md` for product scope.

## Workspace

- `Cargo.toml` — resolver 2, 6 members. Version matrix is pinned (PRD §14): `tokio 1.43 full`, `cpal 0.18`, `rubato 0.15`, `ort 2.0.0-rc.12`, `enigo =0.3` (no upgrades), `zbus 5`, `atspi 0.26`, `notify 8`, `reqwest 0.12 stream+json`, `tokio-tungstenite 0.26`, `eframe/egui 0.36`. Do not bump without updating PRD.
- Crates: `core` (ToolCall/ToolResponse schema + validation, FR-1) → `config` (toml parsing, FR-5) → `dispatcher` (7 tools + whitelist/sandbox, FR-1a) → `voice` (cpal/rubato/ort skeleton) → `net` (reqwest SSE + tungstenite WS) → `cli` (binary `heraldvis`, headless REPL + `--gui` overlay). Dependency direction is `cli` depends on all others; others never depend on `cli`.
- Entrypoint: `crates/cli/src/main.rs` (`heraldvis` binary). Flags: `--check` (dispatcher self-test, no VPS needed), `--gui` (requires `gui` feature + display).
- `config.example.toml` is the source of truth for config schema; `config.toml` is gitignored. `Cargo.lock` is committed (binary workspace).
- Toolchain: `rustc 1.97.1 stable` (verified 2026-09-02). No `rust-toolchain.toml`; uses default.

## Commands (exact)

```bash
cargo check --workspace               # fast verify, headless — must pass without apt dev headers
cargo test --workspace                # 11 suites (core/config/dispatcher/net/voice/cli)
cargo test -p heraldvis-core          # single crate
cargo test -p heraldvis-dispatcher -- --nocapture
cargo run -p heraldvis -- --check     # dispatcher self-test: prints CHECK OK
cargo run -p heraldvis                # headless REPL: pipe ToolCall JSON per line, `exit` to quit
cargo run -p heraldvis --features gui -- --gui  # overlay — requires display + apt (below)
```

Example ToolCall JSON for REPL:
```json
{"name":"write_file","arguments":{"path":"/tmp/heraldvis/hello.txt","content":"hi"}}
```

## System deps & feature gotchas (will break build if missed)

- **Headless WSL passes without apt** — by design: `voice/audio`, `dispatcher/automation`, `cli/gui` are `optional`/`features`. Default workspace build does NOT pull `cpal`/`enigo`/`eframe`, so `cargo check --workspace` works on vanilla WSL with only `libssl3t64`/`libx11-6` runtimes.
- **Full build (audio/GUI/automation) requires apt on Ubuntu desktop** (PRD §14.5, X11 session — Wayland breaks `enigo`/`always-on-top`). Install on the target laptop, not just WSL:
  ```bash
  sudo apt update && sudo apt install -y libasound2-dev libx11-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev pkg-config libssl-dev libxdo-dev
  ```
  Then verify full features:
  ```bash
  cargo check --workspace --features heraldvis-voice/audio,heraldvis-dispatcher/automation,heraldvis/gui
  ```
- **TLS quirk**: workspace `ort` and `reqwest` use `tls-rustls`/`rustls-tls` (via `default-features = false`) so headless does not need `libssl-dev`/`pkg-config`. After `apt install libssl-dev`, `tls-native`/`native-tls` is also valid — change back in `Cargo.toml` if native is preferred. Do not mix both.
- `ort 2.0.0-rc.12` needs `download-binaries` feature; do not remove.

## Config

- Copy `config.example.toml` → `config.toml` (ignored). Fields: `endpoint` (vLLM `/v1/chat/completions`), `api_key`, `mode = text_only|voice`, `[tools]` toggles, `[whitelist] allowed_paths`/`allowed_commands`, `[voice]` models. `cargo run -- --check` validates without needing VPS.

## Repo-specific notes

- Dispatcher is full-auto, no approval gate (FR-1a). Safety = `whitelist` prefix match on `path` and `command` + per-tool `enabled` toggle. When adding tools, update both `core::ToolName` and `dispatcher::Dispatcher::check_whitelist`.
- Generated/locked artifacts: `target/` is ignored; never commit `config.toml`/`*.wav`/`*.onnx`. No CI workflows or `opencode.json` in this repo — `README.md` is the executable setup reference if docs conflict with PRD.

<!-- antislop:start -->
## antislop
For UI, copy, people, mobile layout, or code comments work, read `antislop.md` (core) and then the skill for the task:
- UI / visual: `skills/antislop-ui/SKILL.md`
- Copy & text: `skills/antislop-copywriting/SKILL.md`
- People: `skills/antislop-human/SKILL.md`
- Mobile / responsive: `skills/antislop-layoutmobile/SKILL.md`
- Code comments: `.agents/skills/antislop-code/SKILL.md` ← installed (heraldvis). Follow `antislop-code` hygiene: keep only comments that add info not shown by code; remove decorative banners/empty labels/workflow narration/emoji/end-markers.
Before starting, ask the user when antislop applies: during the work, or after it is done.
<!-- antislop:end -->
