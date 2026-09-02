# Heraldvis — Local Desktop Voice Interpreter for Qwen3.8

> PRD v1.0 (2 Sep 2026) — Client interpreter Rust untuk tool-calling + voice full-duplex di Ubuntu.

## Audit Environment (M1 — Ubuntu 24.04 WSL2)

**Host:** `Linux ST-3ZZ8CL3 6.6.87.2-microsoft-standard-WSL2 #1 SMP` — Ubuntu 24.04.4 LTS (noble)

**Rust toolchain (rustup stable):**
```
rustc 1.97.1 (8bab26f4f 2026-07-14)
cargo 1.97.1 (c980f4866 2026-06-30)
rustup 1.29.0
toolchain: stable-x86_64-unknown-linux-gnu (active, default)
```

Sudah versi terbaru stable — tidak perlu `rustup update`. Jika tertinggal, jalankan:
```bash
rustup update
rustc --version && cargo --version
```

**Status paket sistem apt (PRD §14.5):**
- Runtime lib ada: `libasound2t64`, `libx11-6`, `libxcb-*`, `libssl3t64`
- **Dev headers BELUM ada di WSL ini:** `libasound2-dev`, `libx11-dev`, `libxcb-render0-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libxkbcommon-dev`, `pkg-config`, `libssl-dev`, `libxdo-dev` (untuk `enigo`)
- Tanpa dev headers, `cpal` (ALSA), `eframe` (X11), `enigo` (libxdo), dan `openssl-sys` gagal linker/build (terbukti di `cargo check` awal).

### Instruksi instalasi wajib (Ubuntu Desktop fisik, bukan WSL headless)

Jalankan di laptop Ubuntu 22.04/24.04 target (bukan WSL tanpa display):

```bash
sudo apt update && sudo apt install -y \
  libasound2-dev \
  libx11-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  pkg-config libssl-dev \
  libxdo-dev

# opsional tapi disarankan untuk eframe/Wayland:
sudo apt install -y libwayland-dev libxkbcommon-x11-dev
```

> **Catatan WSL headless:** Repo ini sudah di-optimasi agar `cargo check --workspace` dan `cargo test --workspace` **lolos tanpa apt** di WSL (fitur `audio`/`gui`/`automation` dijadikan optional). Build penuh dengan audio/GUI/enigo tetap butuh paket di atas — enable via:
> ```bash
> cargo check --workspace --features heraldvis-voice/audio,heraldvis-dispatcher/automation,heraldvis/gui
> cargo run -p heraldvis --features gui -- --gui
> ```

> **Display server:** Untuk v1, jalankan sesi Ubuntu di **Xorg (X11)** bukan Wayland (PRD §14.5): `enigo` butuh portal RemoteDesktop di Wayland dan `always-on-top` dibatasi compositor.

## Struktur Workspace (PRD Bab 9 + 14)

```
.
├── Cargo.toml              # workspace root — dependency matrix terkunci Bab 14
├── config.example.toml     # FR-5: endpoint, whitelist, mode, voice
├── crates/
│   ├── core/        # schema ToolCall/ToolResponse, error, validation FR-1 (11 tools)
│   ├── dispatcher/  # 11 tool handlers + whitelist/sandbox FR-1a + full-access FR-1a
│   ├── vision/      # in-memory framebuffer FR-7 (xcap/image/base64, zero disk)
│   ├── voice/       # cpal+rubato+ort(VAD v5)+hound+barge-in FR-2 (M3/M4 full-duplex)
│   ├── net/         # reqwest(SSE)+tokio-tungstenite(WS) FR-3 (M2 SSE typed + reconnect)
│   ├── config/      # toml parsing FR-5 + FR-7c inspect_screen toggle
│   └── cli/         # binary heraldvis — headless + eframe overlay FR-1b (11→ streaming loop 10 iter)
└── PRD.md
```

**Matrix versi terkunci (Bab 14.1/14.3):**
`tokio 1.43 full`, `cpal 0.18`, `rubato 0.15`, `ort 2.0.0-rc.12`, `enigo 0.3 pinned`, `zbus 5.0`, `atspi 0.26`, `notify 8.0`, `reqwest 0.12 stream+json`, `tokio-tungstenite 0.26`, `eframe/egui 0.36`.

> Untuk WSL tanpa `libssl-dev`, `ort` dan `reqwest` di workspace dipatch ke `tls-rustls` agar tidak butuh `pkg-config`+OpenSSL. Setelah `apt install libssl-dev`, varian `tls-native`/`native-tls` juga valid — cukup ubah kembali di `Cargo.toml` jika ingin native.

## Quickstart M1 (text-only dispatcher)

```bash
# 1) cek tanpa butuh VPS
cargo run -p heraldvis -- --check
# -> CHECK OK — dispatcher works: heraldvis check

# 2) REPL ToolCall (pipe JSON per baris)
cargo run -p heraldvis
{"name":"write_file","arguments":{"path":"/tmp/heraldvis/hello.txt","content":"hi"}}
{"name":"read_file","arguments":{"path":"/tmp/heraldvis/hello.txt"}}
{"name":"execute_command","arguments":{"command":"echo hello"}}
exit

# 3) GUI overlay (butuh display X11 + apt)
cargo run -p heraldvis --features gui -- --gui

# 4) validasi
cargo check --workspace
cargo test --workspace
```

## Konfigurasi (FR-5 / FR-5a / FR-6 / FR-7)

Copy `config.example.toml` → `config.toml` dan sesuaikan `endpoint` VPS vLLM/SGLang (`/v1/chat/completions` streaming) dan `whitelist`. Whitelist `allowed_commands` sudah diperluas untuk dataset linux-command 8700 sampel — persempit lagi untuk production (FR-1a full-auto tanpa approval gate, Safety §13.5).

**Precedence FR-5a (highest → lowest):** `CLI flags > env vars > config.toml > fallback default`

- `endpoint`: `--endpoint <URL>` > `HERALDVIS_ENDPOINT` > `config.toml:endpoint` > `http://127.0.0.1:8000`
- `api_key`: `--api-key <KEY>` > `HERALDVIS_API_KEY` > `config.toml:api_key` > (none)
- Jika `api_key` tidak kosong, `heraldvis-net` otomatis kirim `Authorization: Bearer <api_key>` di SSE + WS.

## Menjalankan Binary Rilis (Ubuntu Desktop) — FR-5b

Download artifact dari GitHub Releases (`heraldvis-linux-x86_64.tar.gz`) atau build lokal `cargo build --release --locked`.

```bash
tar -xzvf heraldvis-linux-x86_64.tar.gz
chmod +x heraldvis

# Opsi 1: Export Environment Variables di Bash
# (precedence 2 — override config.toml)
export HERALDVIS_ENDPOINT="http://129.212.186.196:8000"
export HERALDVIS_API_KEY="opsional_token"  # kosongkan jika VPS tanpa auth
./heraldvis
./heraldvis --check            # self-test tanpa VPS

# Opsi 2: Menggunakan CLI Flags Langsung
# (precedence 1 — override env + config)
./heraldvis --endpoint "http://129.212.186.196:8000" --api-key "opsional_token"
./heraldvis --endpoint "http://129.212.186.196:8000" --check

# Opsi 3: Menggunakan config.toml
# (precedence 3)
cp config.toml ./config.toml  # edit endpoint/api_key di file
./heraldvis
```

> Release workflow: `.github/workflows/release.yml` — trigger `push tag v*` atau `workflow_dispatch`, build `ubuntu-latest` (`libasound2-dev` + `cargo build --release --locked`), packaging `heraldvis` + `config.toml` → `heraldvis-linux-x86_64.tar.gz` via `softprops/action-gh-release@v2`.
> `config.toml` di artifact adalah copy dari `config.example.toml` — ganti `endpoint` ke VPS aktif setelah extract.

## Tools (11) — FR-1 + FR-6a + FR-7c

`open_application` · `read_file` · `write_file` · `run_test` · `git_operation` · `execute_command` · `navigate_browser` (xdg-open agnostik) · `press_key` · `type_text` · `take_screenshot` · `inspect_screen` (in-memory JPEG Data URL, detail_level low/high, zero disk) — semua dengan `[tools]` toggle di `config.toml` + whitelist path/command, HEADLESS mock bila tanpa `--features automation/xcap`.

**Full Access:** `--full-access` (CLI) atau `whitelist.enabled=false` bypass sandbox — banner `FULL ACCESS` tampil, log `blocked_by_whitelist=false`, safety tetap audit via SessionLogger JSONL.

## Roadmap

M0 PRD ✓ | M1 skeleton+dispatcher ✓ | M2 WS/SSE ✓ (typed ChatChunk/SseEvent, reconnect jitter) | M3 voice pipeline ✓ (cpal/rubato/ort VAD v5) | M4 barge-in ✓ (full-duplex) | M5 E2E ✓ (SSE stream → sentence TTS queue → tool auto-dispatch, offline fallback) | M6 desktop automation ✓ (press_key/type_text/take_screenshot + autonomous loop 10 iter) | M7 vision ✓ (in-memory framebuffer, inspect_screen low 768/high 1024, zero disk, headless synthetic fallback) + logging SINTA 3 (JSONL /tmp/heraldvis/sessions)

> Training Qwen3.8-27B QLoRA masih `11/2991` step (~3 jam) di VPS MI300X (`PROGRES_LOG.md`) — heraldvis sudah siap uji begitu vLLM/SGLang ROCm serve `http://129.212.186.196:8000/v1` (lihat `config.example.toml`).
