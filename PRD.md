# PRD — Local Desktop Voice Interpreter for Qwen3.8 Desktop Agent

**Versi:** 1.0
**Tanggal:** 2 September 2026
**Status:** Draft — untuk implementasi setelah fine-tuning Qwen3.8-27B selesai
**Konteks proyek:** Bagian dari riset AI + HPC BRIN (Multimodal Autonomous Desktop Agent, target publikasi SINTA 3)

---

## 1. Ringkasan Eksekutif

Software ini adalah **client interpreter** yang berjalan di Ubuntu Linux desktop (laptop lokal), berfungsi sebagai:

1. **Eksekutor tool-calling** — menerima instruksi terstruktur (`tool_call`) dari model Qwen3.8-27B yang di-fine-tune (jalan di VPS GPU AMD MI300X), lalu mengeksekusinya secara nyata di desktop (buka aplikasi, baca/tulis file, jalankan test, operasi git, jalankan command, navigasi browser).
2. **Voice interface real-time** — memungkinkan interaksi suara dua-arah (full-duplex, streaming, low-latency, gaya "telfonan dengan AI" / Jarvis-style), dengan dukungan barge-in (user bisa memotong AI saat AI sedang bicara).

Bahasa interaksi suara: **English only**.

---

## 2. Latar Belakang & Tujuan

- Model Qwen3.8-27B sedang di-fine-tune (QLoRA) khusus untuk tool-calling desktop-agent, dilatih dengan dataset gabungan (~9.200 sampel: desktop agent sintetis + Linux command).
- Model akan di-serve di VPS GPU (rencana: vLLM atau SGLang, native ROCm) setelah training & evaluasi selesai.
- Dibutuhkan software terpisah di sisi client (laptop Ubuntu) untuk:
  - Menjembatani hasil inference model ke aksi nyata di OS lokal (tool execution).
  - Menyediakan pengalaman voice-agent real-time supaya interaksi terasa alami, bukan hanya text-based chat.
- **Tujuan utama:** software siap dipakai untuk **menguji hasil fine-tuning** begitu training selesai — baik lewat teks (tool-calling) maupun suara (voice agent).

---

## 3. Target Pengguna

- Peneliti/penulis proyek sendiri (single-user, untuk keperluan riset & demo/evaluasi hasil fine-tuning).
- Bukan produk multi-user/publik pada fase ini.

---

## 4. Scope

### 4.1 In-Scope (v1)
- Text/tool-calling interpreter (dispatcher) yang mengeksekusi tool-tool berikut sesuai skema dataset training:
  - `open_application`
  - `read_file`
  - `write_file`
  - `run_test`
  - `git_operation`
  - `execute_command`
  - `navigate_browser` / `open_browser`
- Komunikasi client ↔ VPS via WebSocket/HTTP streaming (SSE) ke endpoint OpenAI-compatible (vLLM/SGLang).
- Voice pipeline real-time:
  - Capture audio mic lokal (cpal)
  - VAD lokal (Silero VAD) untuk deteksi start/stop bicara + barge-in
  - STT streaming (Parakeet TDT, dijalankan di VPS, English-only)
  - LLM streaming token (Qwen3.8-27B di VPS)
  - TTS streaming per-kalimat (Kokoro-82M atau Pocket TTS, di VPS)
  - Playback audio lokal (cpal) dengan interrupt handling
- Logging/transkrip percakapan & tool-call untuk keperluan evaluasi riset.
- Konfigurasi via file config (alamat VPS, API key, pilihan tool yang diizinkan/sandboxed).

### 4.2 Out-of-Scope (v1)
- Multi-user / auth kompleks
- Dukungan multilingual voice (fokus English saja)
- Deployment lintas OS (fokus Ubuntu/Linux desktop dulu; Windows/macOS = future work)
- Fine-tuning/training model (sudah dikerjakan terpisah, bukan bagian software ini)
- Approval gate interaktif untuk eksekusi tool (lihat FR-1a — full-auto by design)

---

## 5. Arsitektur Sistem

```
┌─────────────────────────────┐        ┌──────────────────────────────────┐
│   LAPTOP (Ubuntu Desktop)   │        │      VPS GPU (AMD MI300X)         │
│   Rust Client/Daemon        │        │                                    │
│                              │        │  ┌──────────────────────────┐    │
│  ┌────────────────────┐     │  WS/   │  │ Qwen3.8-27B (fine-tuned)  │    │
│  │ Mic (cpal)          │     │  HTTP  │  │ served via vLLM/SGLang   │    │
│  └─────────┬──────────┘     │  SSE   │  └────────────┬─────────────┘    │
│            ▼                │◄──────►│               │                   │
│  ┌────────────────────┐     │        │  ┌────────────▼─────────────┐    │
│  │ Silero VAD          │     │        │  │ Parakeet TDT STT stream  │    │
│  └─────────┬──────────┘     │        │  └───────────────────────────┘    │
│            ▼                │        │  ┌───────────────────────────┐    │
│  ┌────────────────────┐     │        │  │ Kokoro/Pocket TTS stream  │    │
│  │ Tool Dispatcher      │     │        │  └───────────────────────────┘    │
│  │ (D-Bus/AT-SPI, fs,   │     │        │                                    │
│  │  process, git)       │     │        └────────────────────────────────────┘
│  └─────────┬──────────┘     │
│            ▼                │
│  ┌────────────────────┐     │
│  │ Speaker (cpal)       │     │
│  │ + barge-in cancel    │     │
│  └────────────────────┘     │
└─────────────────────────────┘
```

---

## 6. Pilihan Teknologi

| Komponen | Pilihan | Alasan |
|---|---|---|
| Bahasa | **Rust** | Memory-safety untuk eksekusi aksi nyata dari output LLM, ekosistem async matang, distribusi binary tunggal |
| Async runtime | `tokio` | Standar de-facto, dipakai bersama untuk network + audio pipeline |
| Audio I/O | `cpal` | Cross-platform, low-level, real-time-safe |
| VAD | Silero VAD (via `ort`/ONNX Runtime) | Ringan, akurat, standar untuk voice-agent |
| STT | Parakeet TDT 1.1B (server-side di VPS) | RTFx tertinggi untuk streaming real-time, English-only sesuai kebutuhan |
| TTS | Kokoro-82M / Pocket TTS (server-side di VPS) | Ringan, streaming-capable, kualitas baik |
| Desktop automation | `enigo`, `atspi`/`zbus`, `notify` | Kontrol aplikasi & file watching di Linux desktop |
| Serving model | vLLM atau SGLang (ROCm native) | Sudah direncanakan di tahap deployment proyek |
| Referensi arsitektur | crate `skadoosh` (Rust voice agent: VAD→STT→LLM→TTS dgn barge-in) | Pola pipeline serupa, bisa jadi referensi/starting point |

---

## 7. Functional Requirements

### FR-1: Tool Dispatcher
- Menerima `tool_call` JSON sesuai format native chat template Qwen3.8 (`<tool_call><function=...>`).
- Memvalidasi parameter sebelum eksekusi (schema validation).
- Mengeksekusi tool sesuai handler masing-masing, mengembalikan hasil dalam format `<tool_response>`.
- Sandboxing dasar: whitelist path yang boleh diakses `read_file`/`write_file`, whitelist command untuk `execute_command`.

### FR-1a: Eksekusi Full-Auto (Tanpa Approval Gate)
- Semua tool, termasuk `execute_command`, dieksekusi otomatis begitu tool-call valid lolos schema validation — **tidak ada** dialog konfirmasi/approval interaktif dari user.
- Karena tanpa human-in-the-loop, keamanan bergantung penuh pada whitelist/sandboxing di FR-1 — ini jadi titik kritis yang wajib diuji ketat (lihat metrik Safety di §13).
- Setiap eksekusi tetap dicatat lengkap di log (§12) sebagai jejak audit, meski tanpa approval real-time.

### FR-1b: GUI Floating Window (Jarvis-style)
- Software menyediakan **GUI overlay** yang melayang (floating) di atas window lain di Ubuntu desktop, bukan cuma CLI/daemon.
- Window bisa **dipindah-pindah/digeser** bebas ke posisi mana saja di layar (draggable).
- Window bisa **di-minimize** ke ikon kecil/status-indicator mengambang (mirip Jarvis/assistant overlay), dan dibesarkan kembali dengan satu klik.
- Window menampilkan indikator status real-time (listening / thinking / speaking / idle) dan transkrip ringkas.
- Implementasi: kandidat framework Rust GUI yang mendukung frameless + always-on-top + transparansi di Linux (mis. `egui`/`eframe`, atau `tauri` dengan window frameless+transparent). Dipilih lebih lanjut saat masuk fase M3/GUI implementation.

### FR-2: Voice Streaming Pipeline
- Latency end-to-end (di luar jaringan internet) ditarget serendah mungkin: VAD instan, STT chunk ~100-200ms, TTS time-to-first-audio ~100-300ms.
- TTS diputar per-kalimat (clause-split), tidak menunggu seluruh response selesai.
- Barge-in: begitu VAD mendeteksi user mulai bicara saat AI sedang TTS, AI harus berhenti bicara dan mengirim interrupt ke server.

### FR-3: Koneksi ke VPS
- WebSocket persistent connection untuk streaming audio + token.
- Reconnect otomatis jika koneksi putus.
- Autentikasi sederhana (API key/token di config).

### FR-4: Logging & Evaluasi
- Setiap sesi (voice maupun text tool-calling) dicatat: input, tool_call yang dipanggil, hasil eksekusi, response akhir — untuk bahan evaluasi kualitatif di publikasi riset.

### FR-5: Konfigurasi
- File config (`config.toml`) untuk: alamat VPS/endpoint, pilihan tool yang aktif, path yang di-whitelist, pilihan voice (STT/TTS model), mode (text-only vs voice).

### FR-5a: Multi-Tier Configuration & Precedence Hierarchy
- Konfigurasi koneksi (khususnya `endpoint` dan `api_key`) harus mendukung resolusi berlapis dengan urutan prioritas (*highest to lowest*):
  1. **Command Line Flags**: `--endpoint <URL>` dan `--api-key <KEY>`
  2. **Environment Variables**: `HERALDVIS_ENDPOINT` dan `HERALDVIS_API_KEY`
  3. **Configuration File**: nilai pada `config.toml`
  4. **Fallback Default**: `http://127.0.0.1:8000`
- Implementasi resolver di `crates/cli` (atau `crates/config` helper) wajib mengikuti pola:
  ```rust
  let endpoint = cli_endpoint
      .or_else(|| std::env::var("HERALDVIS_ENDPOINT").ok())
      .unwrap_or_else(|| config.endpoint.clone());
  let api_key = cli_api_key
      .or_else(|| std::env::var("HERALDVIS_API_KEY").ok())
      .or_else(|| config.api_key.clone());
  ```
- Jika `api_key` tidak kosong, header `Authorization: Bearer <api_key>` otomatis disisipkan ke request HTTP SSE di `heraldvis-net` (`HeraldvisClient::chat_completions_stream` + `connect_ws`).
- `cargo run -- --help` wajib mendokumentasikan kedua flag (`--endpoint`, `--api-key`).

### FR-5b: Automated Standalone Binary Release via CI/CD
- Menyediakan alur build otomatis di GitHub Actions yang mengompilasi binary rilis Ubuntu x86_64 (`target/release/heraldvis`), mengemasnya bersama `config.example.toml`, dan mempublikasikannya ke GitHub Releases saat tag versi dibuat (misal `v0.1.0`) atau via pemicu manual (`workflow_dispatch`).
- Workflow file: `.github/workflows/release.yml` — trigger `on: push: tags: ['v*']` + `workflow_dispatch`, job `ubuntu-latest`, steps: `actions/checkout@v4` → pasang Rust `stable` → `sudo apt-get update && sudo apt-get install -y libasound2-dev` → `cargo build --release --locked` → packaging `heraldvis-linux-x86_64` dir → `tar -czvf heraldvis-linux-x86_64.tar.gz` → publish via `softprops/action-gh-release@v2`.
- Artifact berisi `heraldvis` binary + `config.toml` (copy dari `config.example.toml`).

---

## 8. Non-Functional Requirements

- **Reliability:** tool execution tidak boleh membuat sistem dalam keadaan rusak (idempotent handler, error handling jelas, tidak silent-fail).
- **Security:** tidak boleh mengeksekusi command arbitrary tanpa whitelist/sandboxing — penting karena input berasal dari output LLM yang bisa tidak terduga.
- **Performance:** binary tunggal, startup cepat, resource footprint kecil di laptop (karena model berat ada di VPS).
- **Observability:** log terstruktur (level: debug/info/warn/error), mudah ditelusuri untuk debugging pipeline voice yang banyak stage-nya.
- **Portability:** target Ubuntu Linux desktop dulu; desain modular supaya OS lain bisa ditambahkan nanti.

---

## 9. Struktur Proyek (Rencana Cargo Workspace)

```
desktop-agent-interpreter/
├── Cargo.toml                 # workspace root
├── crates/
│   ├── core/                  # tipe bersama, schema tool-call, error types
│   ├── dispatcher/            # tool handler: fs, process, git, dbus/atspi, browser
│   ├── voice/                 # cpal capture/playback, VAD client, barge-in logic
│   ├── net/                   # WebSocket/HTTP client ke VPS (vLLM/SGLang endpoint)
│   ├── config/                # parsing config.toml
│   └── cli/                   # entrypoint binary (daemon + floating GUI overlay / optional headless TUI)
├── config.example.toml
└── PRD.md
```

---

## 10. Roadmap / Milestone

| Fase | Deliverable | Estimasi |
|---|---|---|
| M0 | PRD final (dokumen ini) | Selesai |
| M1 | Skeleton Cargo workspace + tool dispatcher untuk text-only tool-calling (tanpa voice) | Setelah training checkpoint pertama siap dievaluasi |
| M2 | Integrasi WebSocket client ke VPS (vLLM/SGLang endpoint streaming) | Paralel M1 |
| M3 | Voice pipeline dasar: cpal + Silero VAD + koneksi STT/TTS server-side | Setelah M1-M2 stabil |
| M4 | Barge-in & full-duplex streaming | Setelah M3 |
| M5 | Uji end-to-end dengan model hasil fine-tuning final | Setelah training + evaluasi model selesai |
| M6 | Logging/evaluasi terstruktur untuk bahan publikasi SINTA 3 | Paralel M5 |

---

## 11. Risiko & Mitigasi

| Risiko | Mitigasi |
|---|---|
| Tool-call dari model salah format/parameter | Schema validation ketat sebelum eksekusi, fallback error response ke model |
| Command eksekusi berbahaya dari output model | Whitelist path & command, sandboxing |
| Latency jaringan antara laptop-VPS tidak stabil | Reconnect logic, buffering audio chunk, degradasi graceful (fallback text) |
| Model belum akurat pasca fine-tuning awal | Logging detail untuk iterasi dataset/training berikutnya |
| Kompleksitas D-Bus/AT-SPI di berbagai desktop environment (GNOME/KDE) | Scope awal fokus 1 DE yang dipakai peneliti, expand kemudian |

---

## 12. Fallback & Approval — Keputusan

- **Fallback text-only:** Ya, dibutuhkan. Kalau koneksi voice (WebSocket ke VPS STT/TTS) tidak stabil, software otomatis degradasi ke mode text-only tanpa memutus sesi — user tetap bisa lanjut lewat input teks di GUI floating window yang sama.
- **Approval gate `execute_command`:** Tidak ada — full-auto sesuai desain riset (lihat FR-1a). Konsekuensinya, whitelist/sandboxing (FR-1) dan metrik Safety (§13) jadi wajib diuji ketat sebelum dipakai untuk evaluasi nyata.

## 13. Metrik Evaluasi untuk Publikasi SINTA 3 (Proposal)

Disusun dari literatur evaluasi LLM tool-calling agent & voice-agent terkini. Dikelompokkan per level pengukuran:

### 13.1 Tool-Calling Correctness (per-panggilan)
| Metrik | Definisi |
|---|---|
| Tool Selection Accuracy | Ketepatan memilih tool yang benar dari toolset yang tersedia |
| Argument/Parameter Correctness | Validitas parameter yang di-generate sesuai skema |
| Tool Invocation Awareness | Ketepatan *kapan* harus memanggil tool vs cukup jawab langsung (relevan dengan kategori "negative examples" di dataset) |

### 13.2 Task-Level Success (trajektori/end-to-end)
| Metrik | Definisi |
|---|---|
| Task Completion / Success Rate | Verifikasi state akhir nyata (file benar tertulis, test benar lulus, commit benar terjadi) — bukan cuma teks output |
| Step Efficiency | Jumlah langkah dipakai vs langkah minimum yang diperlukan |
| Error Recovery Rate | Khusus kategori dataset "error recovery": keberhasilan mendeteksi & memperbaiki error di percobaan berikutnya |

### 13.3 Latency (operasional, di luar faktor internet)
- Time-to-first-token (LLM)
- Time-to-first-audio (TTS)
- STT chunk latency
- End-to-end round-trip (voice-in → voice-out)

### 13.4 Voice-Specific
- **WER (Word Error Rate)** STT pada domain command teknis (nama file, command Linux, dll — biasanya lebih tinggi dari domain general speech)
- **Barge-in responsiveness** — kecepatan sistem berhenti bicara saat user memotong

### 13.5 Safety (kritis karena full-auto tanpa approval gate)
- **Unsafe/out-of-scope action rate** — frekuensi model mencoba memanggil command di luar whitelist; jadi justifikasi keamanan utama di paper untuk desain full-auto

### 13.6 Metodologi Pengukuran
- Task Completion & Error Recovery diukur lewat state verification otomatis (cek file/test/git state), bukan LLM-as-judge, supaya reproducible dan bebas dari rubric-drift/judge-variance yang jadi kelemahan umum di benchmark tool-calling (dicatat literatur sebagai sumber inkonsistensi skor antar-run).
- Argument Correctness & Tool Selection Accuracy diukur otomatis lewat schema/reference matching (bukan LLM-judge) untuk konsistensi.
- Logging (§12/FR-4) perlu mencatat: tool-call lengkap + argumen, hasil eksekusi (sukses/gagal + state sesudah), timestamp per-stage (untuk hitung latency), dan flag kalau kena whitelist block (untuk hitung Safety metric).

## 14. Matrix Dependency & API Terbaru (Riset Context7/Crates.io)

> Update teknis berdasarkan riset dokumentasi resmi & registry crates.io. Pin versi ini saat mulai implementasi (M1 ke atas) — cek ulang jika sudah lewat beberapa bulan dari tanggal PRD ini.

### 14.1 Matrix Versi

| Komponen | Crate | Versi | Catatan |
|---|---|---|---|
| Async Runtime | `tokio` | 1.43+ / 1.53.x | `features = ["full"]` |
| Audio I/O | `cpal` | 0.18.x | Kompatibel ALSA/PulseAudio/PipeWire di Ubuntu |
| Audio Resampling | `rubato` | 0.15.x | Wajib — lihat catatan Sample Rate di §14.2 |
| VAD Inference | `ort` | 2.0.0-rc.12+ | Arsitektur v2 baru, jauh lebih cepat dari v1.x |
| Model VAD | Silero VAD | v5.0 (ONNX) | Input tensor beda dari v4: butuh `input`, `state`, `sr` |
| Desktop Automation | `enigo` | 0.3.x (pinned) | Perombakan API total: trait `Keyboard` & `Mouse` |
| D-Bus Client | `zbus` | 5.x | Macro proxy v5 |
| Accessibility Tree | `atspi` | 0.26+ / 0.30 | Integrasi D-Bus AT-SPI2 |
| File Watching | `notify` | 8.x | Event filter berbasis channel async |
| GUI Overlay (FR-1b) | `eframe`/`egui` | 0.36.x | `egui::ViewportBuilder` untuk window transparan & always-on-top |
| WebSocket Client | `tokio-tungstenite` | 0.26+ | Streaming laptop ↔ VPS |
| HTTP/SSE Client | `reqwest` | 0.12.x | `features = ["stream", "json"]` |

> Catatan: `enigo` punya dua jalur versi (0.3.x dan 0.6.x) yang sama-sama sudah pakai trait `Keyboard`/`Mouse` baru. PRD ini mengunci ke **0.3.x** (selaras dengan Cargo.toml §14.3) — jangan campur versi lain saat scaffolding.

### 14.2 Perubahan & Catatan API Kritis

**`ort` v2 + Silero VAD v5** — session pakai builder pattern langsung (bukan environment manual v1.x), inferensi lewat macro `ort::inputs!`. Silero VAD v5 butuh 3 input tensor: `input` (f32 chunk, misal 512 sampel @16kHz = 32ms), `state` (hidden state recurrent, di-carry-over antar iterasi), `sr` (sampling rate, i64). Output: `output` (probabilitas) + `stateN` (state untuk frame berikutnya).

**Audio sample rate & resampling (`crates/voice`)** — Silero VAD v5 mewajibkan input 16kHz (atau 8kHz) f32, frame 512 sampel (32ms). Mayoritas mic laptop/headset USB di Ubuntu (ALSA/PulseAudio/PipeWire) default-nya 44.1kHz atau 48kHz. Wajib resample capture audio ke 16kHz sebelum masuk `ort` — pakai `rubato` (bukan downsampling linier manual, kualitas dan latensinya lebih terjamin untuk real-time).

**`enigo`** — API lama (`key_sequence()`, instansiasi struct langsung) sudah tidak berlaku. Sekarang wajib import trait `Keyboard` dan `Mouse`; text input via `enigo.text(...)`, key event via `enigo.key(Key::X, Click/Press/Release)`, mouse via `enigo.move_mouse(...)`/`enigo.button(...)`.

**`egui`/`eframe`** — window frameless + transparent + always-on-top diatur lewat `egui::ViewportBuilder` (`with_transparent(true)`, `with_decorations(false)`, `with_always_on_top()`). Drag window custom: area header di-`Sense::drag()`, lalu kirim `ctx.send_viewport_cmd(ViewportCommand::StartDrag)` saat title bar diklik-tahan. Ini langsung mengimplementasikan FR-1b (floating, draggable, minimizable).

**`cpal`** — device enumeration & stream builder berjalan non-blocking via callback. Untuk barge-in (FR-2): audio capture dikumpulkan buffer mono f32 512 sampel → dikirim ke ring buffer/mpsc channel ke thread VAD; saat VAD trigger sinyal interupsi, queue playback speaker langsung di-`clear()` dan stream di-pause/reset.

**Sentence-splitting TTS (edge case domain teknis)** — regex naif berbasis tanda baca (`.` `?` `!`) akan salah potong pada istilah teknis yang sering muncul di respons desktop-agent: nama file (`main.rs`, `PRD.md`), versi (`v1.0`), IP (`127.0.0.1`), path (`cargo.lock`). Pakai regex yang mensyaratkan spasi/akhir-string setelah tanda baca, misal `r"[.!?](\s+|$)"`, dan tambahkan pengecualian untuk pola dengan titik-diikuti-karakter-non-spasi (ekstensi file, angka desimal, IP) supaya tidak dianggap akhir kalimat. Detail final regex/parser dituntaskan saat implementasi M3.

### 14.3 Cargo.toml Workspace (Siap Pakai)

```toml
[workspace]
resolver = "2"
members = [
    "crates/core",
    "crates/dispatcher",
    "crates/voice",
    "crates/net",
    "crates/config",
    "crates/cli",
]

[workspace.dependencies]
# Async Runtime & Utilities
tokio = { version = "1.43", features = ["full"] }
tokio-stream = "0.1"
futures-util = "0.3"
async-trait = "0.1"

# Serialization & Config
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"

# Audio & VAD
cpal = "0.18"
rubato = "0.15"
ort = { version = "2.0.0-rc.12", features = ["download-binaries"] }
hound = "3.5"

# Desktop Automation & Linux OS
enigo = "0.3" # pinned — jangan campur dengan jalur versi 0.6.x
zbus = "5.0"
atspi = "0.26"
notify = "8.0"

# Networking ke VPS (vLLM / SGLang)
reqwest = { version = "0.12", features = ["stream", "json"] }
tokio-tungstenite = "0.26"

# GUI Overlay (Jarvis-style)
eframe = "0.36"
egui = "0.36"

# Tracing & Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

### 14.4 Catatan Integrasi Server-side (VPS AMD MI300X)

- **SGLang/vLLM native ROCm:** endpoint harus expose OpenAI-compatible streaming API (`/v1/chat/completions`, `"stream": true`). `crates/net` parse chunk JSON SSE event langsung.
- **TTS per-kalimat:** gunakan sentence-splitting yang aman untuk istilah teknis (lihat §14.2) supaya buffer token dari Qwen3.8 langsung dikirim ke Kokoro-82M tanpa menunggu full completion — target time-to-first-audio di bawah 300ms.

### 14.5 System Prerequisites (Ubuntu `apt`)

Build environment laptop butuh paket sistem berikut karena mengompilasi audio low-level, GUI native, dan D-Bus:

```bash
sudo apt update && sudo apt install -y \
  libasound2-dev \
  libx11-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  pkg-config libssl-dev
```

| Kebutuhan | Paket | Untuk |
|---|---|---|
| Audio | `libasound2-dev` | `cpal` (ALSA backend) |
| GUI (X11/Wayland) | `libx11-dev`, `libxcb-render0-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libxkbcommon-dev` | `eframe`/`egui` |
| SSL/Networking | `pkg-config`, `libssl-dev` | `reqwest`, `tokio-tungstenite` |

Tambahkan blok ini ke README project saat scaffolding M1.

> **Catatan display server (Wayland vs X11):** Untuk tahap riset/evaluasi v1, disarankan menjalankan sesi Ubuntu Desktop pada Xorg (X11) agar simulasi input `enigo` dan floating window `always-on-top` berjalan tanpa restriksi sandbox Wayland — di GNOME/Mutter (default Ubuntu 22.04/24.04), `enigo` butuh izin RemoteDesktop/InputCapture portal dan window positioning/always-on-top dibatasi ketat oleh compositor.

## 15. Open Questions Tersisa

- Threshold spesifik untuk masing-masing metrik (target Task Completion Rate berapa % untuk dianggap "berhasil" di paper) — perlu ditentukan setelah baseline evaluasi model pertama.
- Apakah perlu baseline pembanding (misal Qwen3.8 base tanpa fine-tuning, atau model lain) untuk klaim kontribusi di paper?