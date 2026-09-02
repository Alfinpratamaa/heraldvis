# Log Sesi — Setup Fine-Tuning Qwen3.8-27B di AMD MI300X

**Tanggal:** 2 September 2026
**Status:** Training loop skala penuh sedang berjalan (~9.200 sampel gabungan, 2.991 step)
**Konteks proyek:** Bagian dari riset AI + HPC BRIN (Multimodal Autonomous Desktop Agent, target publikasi SINTA 3) — modul fine-tuning model untuk integrasi dengan [[desktop-voice-interpreter|software interpreter client]] (lihat `PRD.md`). Mengacu pada panduan [HPC LLM Fine-Tuning, LoRA Merging, and Testing Guide](https://drive.google.com/open?id=1bqkl23a37WzpAyFuSev7COhTNfnLtMgAP2pHX3VHxZQ).

---

## 1. Infrastruktur — AMD Developer Cloud

### 1.1 Pilihan Purchasing Option
- **On-demand GPU droplet** dipilih (bukan Spot) — harga tetap $1.99/jam, SLA-backed, tidak berisiko interupsi.
- Spot lebih murah tapi bisa diinterupsi sewaktu-waktu — cocok untuk workload yang bisa checkpoint, tidak dipilih karena training butuh sesi stabil.

### 1.2 GPU Plan
- 1x AMD Instinct MI300X, VRAM 192GB, $1.99/jam.
- Kredit tersedia: $100 (30 hari) → setara ~50 jam pemakaian.
- Image/Template dipilih: **Unsloth Studio (v2026.8.22)**.

### 1.3 Perbandingan Opsi Image yang Tersedia

| Image | Cocok Untuk |
|---|---|
| **Unsloth Studio ✅** | Fast LoRA/QLoRA fine-tuning — dipilih |
| vLLM | Serving/inference OpenAI-compatible API (dipakai di tahap deployment nanti) |
| Primus | Pretrain/post-train skala besar — overkill untuk LoRA |
| JAX | Numerical computing umum |
| SGLang | Alternatif serving ke vLLM |
| MiniMax-H3 | Text-to-video, tidak relevan |
| Kimi K3 | Model spesifik, tidak relevan |
| PyTorch | Base framework manual |

---

## 2. Spesifikasi Droplet (hasil `check_specs.sh`)

| Komponen | Detail |
|---|---|
| GPU | 1x AMD Instinct MI300X VF, 192GB VRAM (196,288 MB), 304 Compute Units, gfx942, PCIe Gen5 x16 |
| CPU | Intel Xeon Platinum 8568Y+, 20 vCPU |
| RAM | 235 GB |
| Disk | Boot 697GB (ext4) + Scratch 5TB (`/dev/vdc`, **ter-mount ke `/mnt/scratch`**) |
| ROCm | 7.14.60850 |
| OS | Ubuntu 24.04.4 LTS |

> **Catatan:** Banner MOTD Unsloth Studio menyebut "5x MI300X" — ini generik dari image template, bukan cerminan droplet aktual (yang sebenarnya cuma 1 GPU sesuai konfigurasi yang dipesan).

---

## 3. Model yang Dipilih: `unsloth/Qwen3.8-27B-unsloth-bnb-4bit`

### 3.1 Kenapa Bukan Varian Lain?

| Varian | Status | Alasan |
|---|---|---|
| NVFP4 (`unsloth/Qwen3.8-27B-NVFP4`) | ❌ Ditolak | Format kuantisasi eksklusif NVIDIA Blackwell tensor core, tidak kompatibel ROCm/AMD |
| GGUF | ❌ Ditolak | Untuk inference llama.cpp, bukan fine-tuning |
| FP8 | ⚠️ Tidak dipilih | Lebih untuk inference/serving |
| **bnb-4bit** | ✅ **Dipilih** | Format kuantisasi software-level (NF4 + double quant), portable ke ROCm, QLoRA-ready |

### 3.2 Detail Teknis Model
- Arsitektur: `Qwen3_5ForConditionalGeneration` — hybrid: Gated DeltaNet (linear attention) + Gated Attention (full attention), pola 3:1 (3 linear-attention layer diikuti 1 full-attention layer), total 64 layers.
- Vision-language model (ada vision encoder terpisah, native image/video understanding).
- File size: 22.3 GB (`model.safetensors`, sudah dalam bentuk 4-bit).
- Context length native: 262,144 token, extensible s.d. 1,000,000 token.
- Requirement: `transformers >= 5.15.1`.

### 3.3 Perbedaan Full Precision vs bnb-4bit

| Skema | VRAM Dibutuhkan (untuk 27B) |
|---|---|
| Full fine-tuning bf16 | ~150-200+ GB (berisiko OOM di 192GB) |
| LoRA di atas bf16 | ~60-70 GB |
| **QLoRA di atas bnb-4bit** | **~20-30 GB — dipilih, hemat, sisa VRAM besar untuk batch/context besar** |

---

## 4. Setup Environment — Isu & Solusi Awal

### 4.1 Version Mismatch
- `transformers` awal: 5.5.0 → di-upgrade ke **5.16.1** (sesuai requirement model 5.15.1+).
- `unsloth`: 2026.8.22 (sudah cukup baru).
- `bitsandbytes`: 0.50.2.dev0 (sudah cukup baru).

### 4.2 Isu pip "externally-managed-environment"
- **Penyebab:** pip di PATH mengarah ke `/usr/bin/pip` (sistem), bukan pip venv Unsloth Studio.
- **Solusi:** gunakan `python -m pip` secara eksplisit dengan path venv:
  ```
  /root/.unsloth/studio/unsloth_studio/bin/python -m pip install --upgrade transformers
  ```

---

## 5. Validasi Loading Model — BERHASIL ✅

```
==((====))==  Unsloth 2026.8.22: Fast Qwen3_5 patching. Transformers: 5.16.1.
   AMD gfx942 GPU. Num GPUs = 1. Max memory: 191.688 GB.
   Torch: 2.11.0+rocm7.2. ROCm Toolkit: 7.2.26015. Triton: 3.7.1
   Bfloat16 = TRUE. FA [Xformers = None. FA2 = False]
```

- Load time: ~19-52 detik (bervariasi per run).
- VRAM terpakai setelah load: 23.11 GB dari 205.82 GB total (sesuai estimasi awal ~20-30GB).
- Catatan performa: Flash Attention 2 belum aktif (`FA2 = False`) — kernel akselerasi mengandalkan Triton bawaan Unsloth.

### 5.1 Generate Test & Catatan Multimodal Processor
- Model vision-language mengembalikan objek `Processor`. Positional argument pertama `Processor.__call__` adalah `images`, bukan `text`.
- Pemanggilan langsung `tokenizer(prompt_text, ...)` memicu `RuntimeError: Unsupported image file. Only jpeg, png, webp and gif are currently supported.` karena string prompt dianggap path file gambar.
- **Solusi:** untuk inferensi teks murni, wajib ekstrak komponen tokenizer teks internal secara eksplisit:
  ```python
  text_tokenizer = getattr(tokenizer, "tokenizer", tokenizer)
  prompt_text = text_tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
  inputs = text_tokenizer(prompt_text, return_tensors="pt").to("cuda")
  ```
  Penyesuaian serupa juga diterapkan pada `SFTTrainer`.

---

## 6. Setup LoRA — BERHASIL ✅

```
trainable params: 79,691,776 || all params: 27,436,420,336 || trainable%: 0.2905
```

### 6.1 Target Modules yang Dipakai (full-attention + MLP saja)
```python
target_modules = ["q_proj", "k_proj", "v_proj", "o_proj", "gate_proj", "up_proj", "down_proj"]
```

### 6.2 Catatan Penting — Arsitektur Hybrid
Modul lain yang ada di model tapi belum dimasukkan ke LoRA (khusus layer Gated DeltaNet/linear-attention, 48 dari 64 layer):

```
in_proj_a, in_proj_b, in_proj_qkv, in_proj_z, linear_attn, out_proj
```

> **Keputusan:** Full-attention + MLP dipilih sebagai baseline awal untuk memastikan kepatuhan format tool-call. Modul Gated DeltaNet dapat dieksplorasi lebih lanjut jika evaluasi multi-turn reasoning membutuhkan representasi sekuens yang lebih dalam.

### 6.3 Config LoRA yang Dipakai
```python
model = FastLanguageModel.get_peft_model(
    model,
    r=16,
    target_modules=["q_proj", "k_proj", "v_proj", "o_proj", "gate_proj", "up_proj", "down_proj"],
    lora_alpha=16,
    lora_dropout=0,
    bias="none",
    use_gradient_checkpointing="unsloth",
    random_state=3407,
)
```

---

## 7. Format Dataset — Tool-Calling — BERHASIL ✅

Model punya `chat_template.jinja` native yang sudah mendukung tool-calling format terstruktur, cocok dengan skema custom tool-calling framework yang direncanakan di riset (dan yang akan dieksekusi nyata oleh [[desktop-voice-interpreter|software interpreter]]).

### 7.1 Format Tool Call Native Model
```
<tool_call>
<function=nama_fungsi>
<parameter=nama_param>
value
</parameter>
</function>
</tool_call>
```

### 7.2 Struktur Pesan
- `system` — instruksi + daftar tools (JSON schema)
- `assistant` — `<think>reasoning</think>` + tool call (atau jawaban final)
- `tool` role — dibungkus otomatis jadi `<tool_response>` di dalam pesan user

### 7.3 Dataset Contoh Awal (`dataset_sample.json`)
Format per-contoh:
```json
{
  "system": "...",
  "tools": [{"name": "...", "description": "...", "parameters": {...}}],
  "conversations": [
    {"role": "user", "content": "..."},
    {"role": "assistant", "reasoning_content": "...", "content": "", "tool_calls": [{"name": "...", "arguments": {...}}]},
    {"role": "tool", "content": "..."},
    {"role": "assistant", "reasoning_content": "...", "content": "jawaban final"}
  ]
}
```

### 7.4 Pipeline Formatting (`prepare_dataset.py`)
1. Load tokenizer/processor model (tanpa load full weight).
2. Baca dataset JSON mentah.
3. Apply `chat_template.jinja` model (otomatis handle `tools` + `tool_calls` + `reasoning`).
4. Convert ke HuggingFace Dataset, simpan ke `./formatted_dataset` (format Arrow).

**Hasil validasi awal:** 2 contoh dataset (single-tool & multi-step tool-calling) berhasil diformat dengan benar, preview output sesuai format native model.

---

## 8. Konfigurasi Scratch Disk 5TB (`/mnt/scratch`)

Untuk mencegah direktori root (720GB) kehabisan ruang akibat cache Hugging Face, checkpoint training, dan dataset Arrow:

- **Format ext4** (reserved blocks dinonaktifkan untuk maksimalkan kapasitas):
  ```bash
  sudo mkfs.ext4 -m 0 -E lazy_itable_init=0,lazy_journal_init=0 /dev/vdc
  ```
- **Mount:**
  ```bash
  sudo mkdir -p /mnt/scratch
  sudo mount -o noatime /dev/vdc /mnt/scratch
  sudo chown -R $USER:$USER /mnt/scratch
  ```
- **Pengalihan direktori cache sistem:**
  ```bash
  export HF_HOME=/mnt/scratch/hf_cache
  export TMPDIR=/mnt/scratch/tmp
  ```
- **Status:** 5.0 TB terpasang dan aktif digunakan untuk cache, dataset, dan output checkpoint (`df -h /mnt/scratch` → 1% terpakai saat verifikasi awal).

---

## 9. Penyiapan SFTTrainer & Isu API Transformers v5

Penyusunan `train_sft.py` (SFTTrainer + PEFT LoRA) merujuk pada panduan HPC di atas.

- **Isu `warmup_ratio`:** `TypeError: TrainingArguments.__init__() got an unexpected keyword argument 'warmup_ratio'`. Di Transformers 5.16.1, `warmup_ratio` dihapus dan dilebur ke `warmup_steps` — parameter ini kini menerima `float` (rasio, misal `0.05` untuk 5%) maupun `int` (langkah absolut). **Solusi:** ubah `warmup_ratio=0.05` menjadi `warmup_steps=0.05`.
- **Isu urutan impor:** `UserWarning: Unsloth should be imported before [trl, transformers, peft]`. Unsloth perlu menginjeksi patching ke PyTorch sebelum pustaka downstream di-load agar akselerasi kernel aktif. **Solusi:** pindahkan `import unsloth` ke baris paling atas skrip, sebelum `trl`/`transformers`/`peft`.

---

## 10. Lingkungan Eksperimen Interaktif (JupyterLab via SSH Tunneling)

1. Instalasi modul di venv Unsloth Studio:
   ```bash
   /root/.unsloth/studio/unsloth_studio/bin/python -m pip install jupyterlab ipykernel
   ```
2. Pendaftaran kernel khusus:
   ```bash
   /root/.unsloth/studio/unsloth_studio/bin/python -m ipykernel install --name unsloth_studio --display-name "Python (Unsloth Studio)"
   ```
3. Eksekusi server di background (PID 11715):
   ```bash
   nohup /root/.unsloth/studio/unsloth_studio/bin/jupyter lab --no-browser --port=8888 --ip=0.0.0.0 --allow-root > /root/jupyter.log 2>&1 &
   ```
4. SSH port forwarding dari terminal lokal:
   ```bash
   ssh -N -L 8888:127.0.0.1:8888 -i id_ed25519_129_server root@129.212.186.196
   ```
   > Catatan: error `channel open failed: Connection refused` teratasi dengan binding eksplisit ke `127.0.0.1` (bukan `localhost`), menghindari isu resolusi IPv6 dan memastikan tunnel terhubung ke port listener background.
5. Akses via browser lokal: `http://localhost:8888/lab?token=<token>`.

---

## 11. Eksperimen Notebook End-to-End (`experiment_qwen38.ipynb`, Sel 1–8)

Notebook disusun modular ke dalam 8 sel utama:

| Sel | Isi |
|---|---|
| 1 | Validasi GPU MI300X dan import library |
| 2 | Pemuatan model `unsloth/Qwen3.8-27B-unsloth-bnb-4bit` (4-bit, context 4096) |
| 3 | Inisialisasi PEFT LoRA (r=16, alpha=16, target modules q/k/v/o/gate/up/down) |
| 4 | Pemuatan dataset terformat (`load_from_disk`) |
| 5 | Sanity check / baseline generation sebelum training |
| 6 | Konfigurasi `TrainingArguments` (bf16=True, optim="adamw_8bit", checkpointing ke `/mnt/scratch`) |
| 7 | Pelatihan via `SFTTrainer` |
| 8 | Penyimpanan LoRA adapter final dan inferensi post-training |

Seluruh sel 1–8 berhasil dieksekusi tuntas pada baseline dataset, termasuk penanganan isu multimodal processor di Sel 5 (lihat §5.1; penyesuaian serupa diterapkan pada `SFTTrainer` di Sel 6).

---

## 12. Sintesis Dataset & Penggabungan Linux Command

### 12.1 Dataset Sintetis Desktop Agent (`generate_desktop_agent_dataset.py`)
Menghasilkan 500 sampel multi-turn terstruktur pada `dataset_desktop_agent.json`:

| Kategori | Proporsi | Deskripsi |
|---|---|---|
| Single-tool actions | 30% | `open_application`, `read_file`, `open_browser` |
| Software dev workflow | 30% | Pola multi-step berurutan: `read_file` → `write_file` → `run_test` |
| Error recovery / debugging | 20% | Test gagal → analisis error → baca config → perbaiki → test ulang berhasil |
| Git lifecycle | 10% | `git_operation` (`status` → `commit` → `push`) |
| Negative examples | 10% | Pertanyaan teoretis/konseptual — melatih model agar tidak memanggil tool bila tidak perlu |

### 12.2 Integrasi `mecha-org/linux-command-dataset`
Dataset eksternal [mecha-org/linux-command-dataset](https://huggingface.co/datasets/mecha-org/linux-command-dataset) (~8.700 pasangan perintah Linux natural-language) diintegrasikan lewat `merge_datasets.py`:

- Penambahan tool baru ke skema: `execute_command(command="...")`.
- Deduplikasi: hash SHA-256 atas `(normalized_user_prompt, normalized_command)`, membuang duplikasi internal maupun tumpang tindih dengan dataset sintetis.
- **Hasil:** 500 sampel sintetis + ~8.700 sampel Linux command → **~9.200 sampel unik** di `dataset_combined.json`.
- Diproses ulang via `prepare_dataset.py` menjadi format Arrow HuggingFace di `./formatted_dataset`.

---

## 13. Eksekusi Training Skala Penuh & Metrik Monitoring

Dijalankan lewat `experiment_qwen38.ipynb` Sel 7 (`trainer.train()`), pada dataset gabungan (~9.200 sampel).

**Konfigurasi aktif:**

| Parameter | Nilai |
|---|---|
| Model | Qwen3.8-27B (bnb-4bit QLoRA) |
| Dataset | ~9.200 sampel gabungan (Desktop Agent + Linux Commands) |
| Per-device train batch size | 4 |
| Gradient accumulation steps | 2 (effective batch size = 8) |
| Optimizer | AdamW 8-bit |
| Precision | bfloat16 |
| Epochs | 3 (total steps: 2.991) |
| Checkpointing | Setiap 20 steps → `/mnt/scratch/checkpoints/qwen38_27b_lora` |

**Metrik monitoring awal:**
- Status: `[ 11/2991 00:35 < 3:13:27, 0.26 it/s, Epoch 0.01/3 ]` (~0.26 it/s, ~3.8 detik/step)
- Training loss awal: 4.304 → 4.269
- Estimasi durasi penyelesaian: ~3 jam 13 menit
- Estimasi biaya komputasi: ~$6.25 (sangat aman di dalam saldo kredit $100)

---

## 14. Status & Langkah Berikutnya

### 14.1 Sudah Selesai
- [x] Infrastruktur GPU droplet dikonfigurasi & tervalidasi
- [x] Model base (Qwen3.8-27B bnb-4bit) berhasil di-load, VRAM sesuai estimasi
- [x] LoRA adapter berhasil di-setup (80M trainable params dari 27.4B)
- [x] Format dataset tool-calling tervalidasi end-to-end
- [x] Scratch disk 5TB (`/dev/vdc`) diformat dan di-mount ke `/mnt/scratch`
- [x] Pipeline SFTTrainer & TrainingArguments disesuaikan untuk Transformers v5.x
- [x] Lingkungan interaktif JupyterLab diaktifkan via SSH tunneling
- [x] Sel 1–8 notebook divalidasi end-to-end (termasuk penanganan isu multimodal processor)
- [x] Generator dataset sintetis desktop agent dibuat (500 sampel: single-tool, workflow, error recovery, git, negatif)
- [x] Dataset eksternal Linux Command digabungkan dengan deduplikasi SHA-256 (~9.200 sampel unik)
- [x] Training skala penuh sedang berjalan pada dataset gabungan (2.991 steps, 3 epochs)

### 14.2 Langkah Berikutnya
- [/] Pantau penyelesaian proses training (2.991 steps, estimasi ~3 jam 13 menit)
- [ ] Verifikasi integritas checkpoint adapter di `/mnt/scratch/checkpoints/qwen38_27b_lora`
- [ ] Evaluasi performa adapter terlatih terhadap skenario tool-calling baru (Sel 8)
- [ ] Upload adapter LoRA final ke Hugging Face Hub (private repo) sebagai backup permanen
- [ ] Persiapan serving inferensi (vLLM/SGLang native ROCm) untuk pengujian integrasi dengan interpreter client
- [ ] Keputusan yang masih terbuka: apakah LoRA perlu mencakup modul linear-attention (Gated DeltaNet), aktivasi Flash Attention 2, strategi dataset sintetis lanjutan

---

## 15. Lampiran — File yang Dibuat & Dimutakhirkan Sepanjang Sesi

| File | Fungsi |
|---|---|
| `check_specs.sh` | Script cek spesifikasi hardware, GPU ROCm, memori, dan storage droplet |
| `test_load_qwen38.py` | Script standalone test loading model Qwen3.8 bnb-4bit |
| `setup_lora_qwen38.py` | Script konfigurasi target modules adapter PEFT LoRA |
| `dataset_sample.json` | Dataset prototipe awal (2 skenario tool-calling) |
| `prepare_dataset.py` | Script format dataset mentah ke Arrow HF Dataset dengan native chat template |
| `train_sft.py` | Script standalone pelatihan SFTTrainer terintegrasi dengan scratch disk |
| `experiment_qwen38.ipynb` | Notebook Jupyter interaktif 8 sel (load, lora, baseline, training, save, eval) |
| `generate_desktop_agent_dataset.py` | Generator sintetis 500 sampel skenario desktop agent multi-turn |
| `merge_datasets.py` | Script penggabungan dataset sintetis dan Linux commands dengan deduplikasi SHA-256 |
| `dataset_combined.json` | Dataset gabungan final bebas duplikat (~9.200 sampel), sedang dilatih |

---

## 16. Sinkronisasi Heraldvis Client (2 Sep 2026 — M2-M5 Selesai)

> Update codebase `heraldvis` Rust client setelah training jalan 11/2991 step. Sinkron dengan dataset & format tool-calling PROGRES_LOG §7/§12.

### 16.1 Heraldvis M2-M5 — Status
- **M2 net WS/SSE** (`heraldvis-net`): typed `ChatChunk/ChatDelta/ToolCallDelta`, `SseEvent`, `parse_sse_line/sse_bytes_to_events`, `ChatStream` via `reqwest`+`async-stream`, `WsConnection` + `connect_ws` (bearer `http::Request`) + `connect_ws_with_reconnect` exponential backoff 30s+jitter. `NetConfig` `ws_url/chat_completions_url/reconnect_base_delay_ms/request_timeout_secs`.
- **M3/M4 voice** (`heraldvis-voice`): `VoiceConfig` + `VoiceError/VadFrameResult/PlaybackQueue` barge-in clear, `resample_linear` + `resample_to_16k` (rubato `SincFixedOut` 512 frame under `audio`), mock energy VAD + `SileroVad` ort state 256 sr16k `inputs!` v5, `VoicePipeline` `start_capture/stop_capture/barge_in/enqueue_pcm/drain_playback/process_vad_frame/split_sentences` §14.2.
- **M5 CLI full-duplex** (`heraldvis` binary): `SessionLogger` JSONL FR-4 `/tmp/heraldvis/sessions`, `openai_tools_schema` 7 tools + `build_chat_payload`, `ToolCallAccum` delta, `placeholder_pcm_for_sentence` sine mock, `run_chat_turn` SSE stream → sentence-split enqueue → tool auto-dispatch `Dispatcher` whitelist, `run_headless` dual ToolCall JSON direct + plain-text LLM stream offline fallback PRD §12, `--voice/--endpoint` overrides, WS `/ws/audio` jitter loop, GUI live `Idle/Listening/Thinking/Speaking`.
- **Deps**: `async-stream 0.3/http 1.3` (net), `ndarray 0.16` (voice), `futures-util` (cli). `cargo check --workspace` ok (headless), `cargo test --workspace` 24 passed. `graphify` 547 nodes/898 edges.
- **Git**: `6259589` M2-M4 + `8e7893d` M5 + `2853ff1` `.gitignore` (ignore `/.opencode/ /graphify-out/ PROGRES_LOG.md`). `origin/main` clean.

### 16.2 Sinkronisasi dengan Dataset & PRD
- Dataset `execute_command` (PROGRES_LOG §12.2) sudah tercover `ToolName::ExecuteCommand` + dispatcher `run_command` via `sh -c` + whitelist `allowed_commands` — `config.example.toml` diperluas 9→24 command (`grep find awk sed mkdir rm chmod cp mv head tail wc sort xargs rg`) untuk eval 8700 sampel linux-command.
- Endpoint VPS `http://129.212.186.196:8000/v1` (droplet MI300X, PROGRES_LOG §1-2) ditambahkan sebagai comment di `config.example.toml` + `README.md` roadmap M2-M5.
- Format `<tool_call>` native Qwen → OpenAI `tool_calls` JSON via vLLM chat_template — tidak perlu parser XML tambahan di `core::ToolCall::from_json` (SSE delta sudah handle).
- **Safety**: FR-1a full-auto tanpa approval gate tetap whitelist-gated; `README` ditegaskan persempit whitelist untuk production setelah eval paper (metrik Safety §13.5 `Unsafe/out-of-scope action rate`).

### 16.3 Langkah Berikutnya (Client)
- [ ] Tunggu training selesai (2.991 steps) → verifikasi checkpoint `/mnt/scratch/checkpoints/qwen38_27b_lora` → upload HF Hub → serve vLLM/SGLang ROCm di `129.212.186.196:8000`.
- [ ] Uji E2E nyata heraldvis ↔ VPS (voice STT Parakeet + LLM Qwen 27B + TTS Kokoro per-kalimat) — ganti `endpoint` di `config.toml`.
- [ ] Evaluasi §13 metrik (Tool Selection / Task Completion / WER / Barge-in / Safety) dengan dataset 9200 sampel, log JSONL `SessionLogger`.
- [ ] Pertimbangan LoRA `in_proj_*` linear-attention (48/64 layer Gated DeltaNet) jika E2E reasoning kurang.