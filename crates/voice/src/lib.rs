//! heraldvis-voice — cpal capture/playback, VAD, barge-in (PRD FR-2, §14.2).
//! M3: cpal + rubato + ort Silero VAD v5. M4: full-duplex + barge-in.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_lossless, clippy::missing_panics_doc, clippy::missing_errors_doc, clippy::comparison_chain, clippy::doc_markdown)]

use std::collections::VecDeque;
use std::path::PathBuf;
use tracing::{info, warn};

/// Konfigurasi audio capture — sampel 16kHz mono f32, frame 512 (32ms) untuk Silero VAD v5.
#[derive(Debug, Clone)]
pub struct VoiceConfig {
    pub sample_rate: u32,
    pub frame_size: usize,
    pub vad_threshold: f32,
    /// Model ONNX Silero VAD v5 (None → mock energy-based VAD for tests/headless)
    pub vad_model_path: Option<PathBuf>,
    /// Input device name (None → default)
    pub input_device: Option<String>,
    /// Output device name (None → default)
    pub output_device: Option<String>,
    /// Native input sample rate before resampling (48k typical for ALSA/Pulse)
    pub input_sample_rate: u32,
    /// Max playback queue frames before dropping oldest (backpressure)
    pub max_playback_frames: usize,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            frame_size: 512,
            vad_threshold: 0.5,
            vad_model_path: None,
            input_device: None,
            output_device: None,
            input_sample_rate: 48_000,
            max_playback_frames: 16_000 * 30, // 30s @16k
        }
    }
}

/// Errors from voice pipeline
#[derive(Debug)]
pub enum VoiceError {
    Device(String),
    Resample(String),
    Vad(String),
    Playback(String),
}

impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Device(m) => write!(f, "device error: {m}"),
            Self::Resample(m) => write!(f, "resample error: {m}"),
            Self::Vad(m) => write!(f, "vad error: {m}"),
            Self::Playback(m) => write!(f, "playback error: {m}"),
        }
    }
}
impl std::error::Error for VoiceError {}

/// Status pipeline voice (untuk GUI indicator FR-1b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    Idle,
    Listening,
    Thinking,
    Speaking,
}

/// Result of single VAD frame
#[derive(Debug, Clone, Copy)]
pub struct VadFrameResult {
    /// Probability speech ∈ [0,1]
    pub prob: f32,
    /// Above threshold?
    pub is_speech: bool,
}

// ---------------------------------------------------------------------------
// Playback queue with barge-in clearing (M4 full-duplex)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct PlaybackQueue {
    buf: VecDeque<f32>,
    interrupted: bool,
}

impl PlaybackQueue {
    fn enqueue(&mut self, pcm: Vec<f32>, cap: usize) {
        for s in pcm {
            if self.buf.len() >= cap {
                self.buf.pop_front();
            }
            self.buf.push_back(s);
        }
        self.interrupted = false;
    }

    fn drain(&mut self, n: usize) -> Vec<f32> {
        let take = n.min(self.buf.len());
        self.buf.drain(..take).collect()
    }

    fn clear(&mut self) {
        self.buf.clear();
        self.interrupted = true;
    }

    fn len(&self) -> usize {
        self.buf.len()
    }

    fn is_interrupted(&self) -> bool {
        self.interrupted
    }
}

// ---------------------------------------------------------------------------
// Resampling: 48k/44.1k → 16k mono
// Pure fallback (linear) always available; rubato path under `audio` feature.
// ---------------------------------------------------------------------------

/// Linear interpolation fallback — no deps, used for headless tests and when rubato unavailable.
/// Quality lower but deterministic and allocation-free per call (outside vec).
#[must_use]
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = (input.len() as f64 * ratio).round() as usize;
    if out_len == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let s0 = input[idx.min(input.len() - 1)];
        let s1 = input[(idx + 1).min(input.len() - 1)];
        out.push(s0 * (1.0 - frac) + s1 * frac);
    }
    out
}

/// Resample arbitrary input to exactly `target_frames` @16k using rubato when `audio` feature enabled.
/// Falls back to linear if rubato fails or feature disabled.
#[must_use]
pub fn resample_to_16k(input: &[f32], from_rate: u32, target_frames: usize) -> Vec<f32> {
    if from_rate == 16_000 {
        // trim/pad to exact
        if input.len() == target_frames {
            return input.to_vec();
        }
        if input.len() > target_frames {
            return input[..target_frames].to_vec();
        }
        let mut out = input.to_vec();
        out.resize(target_frames, 0.0);
        return out;
    }
    #[cfg(feature = "audio")]
    {
        if let Some(v) = try_rubato_resample(input, from_rate, 16_000, target_frames) {
            return v;
        }
    }
    // fallback linear
    let tmp = resample_linear(input, from_rate, 16_000);
    if tmp.len() == target_frames {
        tmp
    } else if tmp.len() > target_frames {
        tmp[..target_frames].to_vec()
    } else {
        let mut out = tmp;
        out.resize(target_frames, 0.0);
        out
    }
}

#[cfg(feature = "audio")]
fn try_rubato_resample(
    input: &[f32],
    from_rate: u32,
    to_rate: u32,
    target_frames: usize,
) -> Option<Vec<f32>> {
    use rubato::{Resampler, SincFixedOut, SincInterpolationParameters, SincInterpolationType, WindowFunction};
    let ratio = to_rate as f64 / from_rate as f64;
    let params = SincInterpolationParameters {
        sinc_len: 64,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 32,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = SincFixedOut::<f32>::new(ratio, 2.0, params, target_frames, 1).ok()?;
    let needed = resampler.input_frames_next();
    // Prepare input padded/truncated to needed
    let mut in_buf = vec![0.0f32; needed];
    let copy_len = needed.min(input.len());
    in_buf[..copy_len].copy_from_slice(&input[..copy_len]);
    let waves_in = vec![in_buf];
    let waves_out = resampler.process(&waves_in, None).ok()?;
    let out = waves_out.into_iter().next()?;
    // Ensure exact target_frames
    if out.len() == target_frames {
        Some(out)
    } else if out.len() > target_frames {
        Some(out[..target_frames].to_vec())
    } else {
        let mut v = out;
        v.resize(target_frames, 0.0);
        Some(v)
    }
}

// ---------------------------------------------------------------------------
// VAD — Silero v5 wrapper (ort) with mock fallback
// ---------------------------------------------------------------------------

/// Mock energy-based VAD for tests/headless when ONNX model absent.
fn mock_vad_prob(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let energy: f32 = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
    // energy 1e-4 → prob ~0.2, 1e-3 → ~0.7, 1e-2 → ~0.95
    let log_e = energy.log10();
    // linear map log_e -4..-1 → 0..1
    ((log_e + 4.0) / 3.0).clamp(0.0, 1.0)
}

#[cfg(feature = "audio")]
struct SileroVad {
    session: ort::session::Session,
    state: Vec<f32>, // [2,1,128] = 256
    sr: i64,
}

#[cfg(feature = "audio")]
impl SileroVad {
    fn new(model_path: &std::path::Path) -> Result<Self, VoiceError> {
        let session = ort::session::Session::builder()
            .map_err(|e| VoiceError::Vad(e.to_string()))?
            .commit_from_file(model_path)
            .map_err(|e| VoiceError::Vad(format!("load {model_path:?}: {e}")))?;
        Ok(Self {
            session,
            state: vec![0.0; 256],
            sr: 16_000,
        })
    }

    fn predict(&mut self, frame: &[f32; 512], threshold: f32) -> Result<VadFrameResult, VoiceError> {
        use ort::inputs;
        // Silero v5 inputs: input [1,512], state [2,1,128], sr [1]
        let input_arr = ndarray::Array2::from_shape_vec((1, 512), frame.to_vec())
            .map_err(|e| VoiceError::Vad(e.to_string()))?;
        let state_arr = ndarray::Array3::from_shape_vec((2, 1, 128), self.state.clone())
            .map_err(|e| VoiceError::Vad(e.to_string()))?;
        let sr_arr = ndarray::arr1(&[self.sr]);
        let outputs = self
            .session
            .run(inputs!["input" => input_arr, "state" => state_arr, "sr" => sr_arr].unwrap())
            .map_err(|e| VoiceError::Vad(e.to_string()))?;
        // output: prob [1,1], stateN [2,1,128]
        let prob: f32 = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| VoiceError::Vad(e.to_string()))?
            .iter()
            .next()
            .copied()
            .unwrap_or(0.0);
        if let Ok(state_n) = outputs["stateN"].try_extract_tensor::<f32>() {
            self.state = state_n.iter().copied().collect();
        }
        Ok(VadFrameResult {
            prob,
            is_speech: prob > threshold,
        })
    }

    fn reset(&mut self) {
        self.state.fill(0.0);
    }
}

// ---------------------------------------------------------------------------
// VoicePipeline — orchestration M3/M4
// ---------------------------------------------------------------------------

pub struct VoicePipeline {
    config: VoiceConfig,
    status: VoiceStatus,
    playback: PlaybackQueue,
    capturing: bool,
    #[cfg(feature = "audio")]
    vad: Option<SileroVad>,
    #[cfg(feature = "audio")]
    _capture_stream: Option<cpal::Stream>,
    #[cfg(feature = "audio")]
    _playback_stream: Option<cpal::Stream>,
}

impl VoicePipeline {
    #[must_use]
    pub fn new(config: VoiceConfig) -> Self {
        #[cfg(feature = "audio")]
        let vad = config
            .vad_model_path
            .as_ref()
            .and_then(|p| SileroVad::new(p).ok());
        Self {
            config,
            status: VoiceStatus::Idle,
            playback: PlaybackQueue::default(),
            capturing: false,
            #[cfg(feature = "audio")]
            vad,
            #[cfg(feature = "audio")]
            _capture_stream: None,
            #[cfg(feature = "audio")]
            _playback_stream: None,
        }
    }

    #[must_use]
    pub fn status(&self) -> VoiceStatus {
        self.status
    }

    #[must_use]
    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    #[must_use]
    pub fn playback_len(&self) -> usize {
        self.playback.len()
    }

    #[must_use]
    pub fn is_interrupted(&self) -> bool {
        self.playback.is_interrupted()
    }

    /// Start capture (cpal) — M1 logs only, no device opened so `cargo check` passes on headless WSL.
    /// M3: when `audio` feature enabled, opens real cpal InputStream + resampler.
    pub fn start_capture(&mut self) {
        info!(
            sample_rate = self.config.sample_rate,
            frame_size = self.config.frame_size,
            "voice capture started (M1 skeleton — no device opened)"
        );
        self.status = VoiceStatus::Listening;
        self.capturing = true;
        #[cfg(feature = "audio")]
        {
            if let Err(e) = self.start_capture_inner() {
                warn!(error = %e, "cpal capture failed, staying in mock mode");
            }
        }
        let _ = Self::ensure_audio_deps_linked();
    }

    pub fn stop_capture(&mut self) {
        info!("voice capture stopped");
        self.status = VoiceStatus::Idle;
        self.capturing = false;
        #[cfg(feature = "audio")]
        {
            self._capture_stream = None;
            if let Some(vad) = self.vad.as_mut() {
                vad.reset();
            }
        }
    }

    /// Barge-in: hentikan playback speaker saat VAD trigger (FR-2, M4 full-duplex).
    /// Clears queue and switches Speaking → Listening.
    pub fn barge_in(&mut self) {
        let had_playback = self.playback.len() > 0;
        if had_playback || self.status == VoiceStatus::Speaking {
            warn!("barge-in triggered — clearing playback queue");
            self.playback.clear();
            #[cfg(feature = "audio")]
            {
                // dropping stream would silence; we keep stream but clear queue
            }
            if self.status == VoiceStatus::Speaking {
                self.status = VoiceStatus::Listening;
            }
        }
    }

    /// Enqueue PCM for playback (16k mono f32). Backpressure: drops oldest if over cap.
    pub fn enqueue_pcm(&mut self, pcm: Vec<f32>) {
        if pcm.is_empty() {
            return;
        }
        self.playback.enqueue(pcm, self.config.max_playback_frames);
        if self.status != VoiceStatus::Speaking {
            self.status = VoiceStatus::Speaking;
        }
    }

    /// Drain up to n samples for output callback.
    pub fn drain_playback(&mut self, n: usize) -> Vec<f32> {
        let out = self.playback.drain(n);
        if self.playback.len() == 0 && self.status == VoiceStatus::Speaking {
            self.status = VoiceStatus::Idle;
        }
        out
    }

    /// Clear playback queue without changing status (low-level).
    pub fn clear_playback_queue(&mut self) {
        self.playback.clear();
    }

    /// Process single 512-frame @16k through VAD, returning prob.
    /// If prob > threshold and currently Speaking, triggers barge-in (M4).
    pub fn process_vad_frame(&mut self, frame: &[f32]) -> VadFrameResult {
        assert_eq!(frame.len(), 512, "VAD frame must be 512 samples @16k");
        let res = {
            #[cfg(feature = "audio")]
            {
                if let Some(vad) = self.vad.as_mut() {
                    let mut arr = [0.0f32; 512];
                    arr.copy_from_slice(frame);
                    vad.predict(&arr, self.config.vad_threshold).unwrap_or_else(|e| {
                        warn!(error=%e, "vad predict failed, fallback mock");
                        let p = mock_vad_prob(frame);
                        VadFrameResult { prob: p, is_speech: p > self.config.vad_threshold }
                    })
                } else {
                    let p = mock_vad_prob(frame);
                    VadFrameResult { prob: p, is_speech: p > self.config.vad_threshold }
                }
            }
            #[cfg(not(feature = "audio"))]
            {
                let p = mock_vad_prob(frame);
                VadFrameResult { prob: p, is_speech: p > self.config.vad_threshold }
            }
        };
        if res.is_speech && self.status == VoiceStatus::Speaking {
            self.barge_in();
        }
        res
    }

    /// Resample incoming native-rate chunk to 16k/512 frames and run VAD on each.
    /// Returns per-frame results; triggers barge-in as side effect if needed.
    pub fn process_resampled_frames(&mut self, input: &[f32], from_rate: u32) -> Vec<VadFrameResult> {
        let resampled = resample_to_16k(input, from_rate, 512);
        // May produce only one frame; for longer inputs chunk into 512
        let mut out = Vec::new();
        for chunk in resampled.chunks(512) {
            if chunk.len() < 512 {
                let mut padded = chunk.to_vec();
                padded.resize(512, 0.0);
                out.push(self.process_vad_frame(&padded));
            } else {
                out.push(self.process_vad_frame(chunk));
            }
        }
        out
    }

    /// Sentence-split aman untuk istilah teknis (PRD §14.2) — dipakai M3 untuk TTS per-kalimat.
    #[must_use]
    pub fn split_sentences(text: &str) -> Vec<String> {
        let mut sentences = Vec::new();
        let mut start = 0;
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut i = 0;
        while i < n {
            let c = chars[i];
            if matches!(c, '.' | '!' | '?') {
                let next_is_space_or_end = i + 1 >= n || chars[i + 1].is_whitespace();
                let is_inside_word = c == '.'
                    && i > 0
                    && i + 1 < n
                    && chars[i - 1].is_alphanumeric()
                    && chars[i + 1].is_alphanumeric();
                if next_is_space_or_end && !is_inside_word {
                    let sentence: String = chars[start..=i].iter().collect();
                    let trimmed = sentence.trim().to_string();
                    if !trimmed.is_empty() {
                        sentences.push(trimmed);
                    }
                    i += 1;
                    while i < n && chars[i].is_whitespace() {
                        i += 1;
                    }
                    start = i;
                    continue;
                }
            }
            i += 1;
        }
        if start < n {
            let tail: String = chars[start..].iter().collect();
            let trimmed = tail.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
        }
        sentences
    }

    // Keep audio/VAD crates linked even when `audio` feature is disabled on headless WSL.
    fn ensure_audio_deps_linked() -> bool {
        #[cfg(feature = "audio")]
        {
            let _ = std::any::type_name::<cpal::Device>();
            let _ = std::any::type_name::<rubato::FftFixedIn<f32>>();
            let _ = std::any::type_name::<ort::session::Session>();
            let _ = std::any::type_name::<hound::WavReader<std::io::Cursor<Vec<u8>>>>();
        }
        true
    }

    #[cfg(feature = "audio")]
    fn start_capture_inner(&mut self) -> Result<(), VoiceError> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = if let Some(name) = &self.config.input_device {
            host.input_devices()
                .map_err(|e| VoiceError::Device(e.to_string()))?
                .find(|d| d.name().map(|n| n == *name).unwrap_or(false))
                .ok_or_else(|| VoiceError::Device(format!("input device not found: {name}")))?
        } else {
            host.default_input_device()
                .ok_or_else(|| VoiceError::Device("no default input device".into()))?
        };
        let supported = device
            .default_input_config()
            .map_err(|e| VoiceError::Device(e.to_string()))?;
        info!(device=?device.name(), config=?supported, "cpal input opened");
        // We create a minimal stream that just logs; real pipeline would channel to VAD.
        // Keep stream alive in _capture_stream so it doesn't drop.
        let err_fn = |err| warn!(error=?err, "cpal input error");
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &supported.into(),
                |_data: &[f32], _| {},
                err_fn,
                None,
            ).map_err(|e| VoiceError::Device(e.to_string()))?,
            cpal::SampleFormat::I16 => device.build_input_stream(
                &supported.into(),
                |_data: &[i16], _| {},
                err_fn,
                None,
            ).map_err(|e| VoiceError::Device(e.to_string()))?,
            cpal::SampleFormat::U16 => device.build_input_stream(
                &supported.into(),
                |_data: &[u16], _| {},
                err_fn,
                None,
            ).map_err(|e| VoiceError::Device(e.to_string()))?,
            _ => return Err(VoiceError::Device("unsupported sample format".into())),
        };
        stream.play().map_err(|e| VoiceError::Device(e.to_string()))?;
        self._capture_stream = Some(stream);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_split_technical_terms() {
        let text = "Check main.rs and PRD.md. Version v1.0 is ready! IP 127.0.0.1 ok?";
        let s = VoicePipeline::split_sentences(text);
        assert_eq!(s.len(), 3);
        assert!(s[0].contains("main.rs"));
        assert!(s[1].contains("v1.0"));
    }

    #[test]
    fn sentence_split_simple() {
        let s = VoicePipeline::split_sentences("Hello world. How are you? Fine!");
        assert_eq!(s, vec!["Hello world.", "How are you?", "Fine!"]);
    }

    #[test]
    fn pipeline_status_transitions() {
        let mut p = VoicePipeline::new(VoiceConfig::default());
        assert_eq!(p.status(), VoiceStatus::Idle);
        p.start_capture();
        assert_eq!(p.status(), VoiceStatus::Listening);
        assert!(p.is_capturing());
        p.barge_in(); // no-op saat listening + empty queue
        assert_eq!(p.status(), VoiceStatus::Listening);
        p.stop_capture();
        assert_eq!(p.status(), VoiceStatus::Idle);
        assert!(!p.is_capturing());
    }

    #[test]
    fn barge_in_clears_playback_and_switches() {
        let mut p = VoicePipeline::new(VoiceConfig::default());
        p.enqueue_pcm(vec![0.1; 1024]);
        assert_eq!(p.status(), VoiceStatus::Speaking);
        assert_eq!(p.playback_len(), 1024);
        // Simulate VAD trigger while speaking → barge_in
        p.barge_in();
        assert_eq!(p.playback_len(), 0);
        assert!(p.is_interrupted());
        assert_eq!(p.status(), VoiceStatus::Listening);
    }

    #[test]
    fn enqueue_and_drain_playback() {
        let mut p = VoicePipeline::new(VoiceConfig::default());
        p.enqueue_pcm(vec![1.0; 100]);
        p.enqueue_pcm(vec![2.0; 50]);
        assert_eq!(p.playback_len(), 150);
        let d = p.drain_playback(100);
        assert_eq!(d.len(), 100);
        assert_eq!(p.playback_len(), 50);
        let d2 = p.drain_playback(100);
        assert_eq!(d2.len(), 50);
        assert_eq!(p.status(), VoiceStatus::Idle); // auto idle when drained
    }

    #[test]
    fn resample_linear_48_to_16() {
        let input = vec![1.0f32; 480];
        let out = resample_linear(&input, 48_000, 16_000);
        assert_eq!(out.len(), 160);
        assert!(out.iter().all(|&v| (v - 1.0).abs() < 1e-5));
    }

    #[test]
    fn resample_to_16k_exact() {
        let input = vec![0.5f32; 512];
        let out = resample_to_16k(&input, 16_000, 512);
        assert_eq!(out.len(), 512);
    }

    #[test]
    fn mock_vad_silence_vs_speech() {
        let silence = vec![0.0f32; 512];
        let mut p = VoicePipeline::new(VoiceConfig { vad_threshold: 0.5, ..Default::default() });
        let r0 = p.process_vad_frame(&silence);
        assert!(!r0.is_speech);
        assert!(r0.prob < 0.1);
        let loud = vec![0.3f32; 512];
        let r1 = p.process_vad_frame(&loud);
        assert!(r1.is_speech);
        assert!(r1.prob > 0.5);
    }

    #[test]
    fn vad_triggers_barge_in() {
        let mut p = VoicePipeline::new(VoiceConfig { vad_threshold: 0.3, ..Default::default() });
        p.enqueue_pcm(vec![0.9; 2048]);
        assert_eq!(p.status(), VoiceStatus::Speaking);
        let loud = vec![0.5f32; 512];
        let r = p.process_vad_frame(&loud);
        assert!(r.is_speech);
        assert_eq!(p.status(), VoiceStatus::Listening);
        assert_eq!(p.playback_len(), 0);
    }

    #[test]
    fn playback_backpressure_capped() {
        let mut p = VoicePipeline::new(VoiceConfig { max_playback_frames: 100, ..Default::default() });
        p.enqueue_pcm(vec![1.0; 80]);
        p.enqueue_pcm(vec![2.0; 50]); // should drop oldest 30
        assert_eq!(p.playback_len(), 100);
    }
}
