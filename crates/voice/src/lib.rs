//! heraldvis-voice — cpal capture/playback, VAD, barge-in (PRD FR-2, §14.2).
//! M1: skeleton — API surface siap, implementasi penuh di M3/M4.

use tracing::{info, warn};

/// Konfigurasi audio capture — sampel 16kHz mono f32, frame 512 (32ms) untuk Silero VAD v5.
#[derive(Debug, Clone)]
pub struct VoiceConfig {
    pub sample_rate: u32,
    pub frame_size: usize,
    pub vad_threshold: f32,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            frame_size: 512,
            vad_threshold: 0.5,
        }
    }
}

/// Status pipeline voice (untuk GUI indicator FR-1b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    Idle,
    Listening,
    Thinking,
    Speaking,
}

/// Placeholder pipeline — M3 akan isi dengan cpal + rubato + ort.
pub struct VoicePipeline {
    config: VoiceConfig,
    status: VoiceStatus,
}

impl VoicePipeline {
    #[must_use]
    pub fn new(config: VoiceConfig) -> Self {
        Self {
            config,
            status: VoiceStatus::Idle,
        }
    }

    #[must_use]
    pub fn status(&self) -> VoiceStatus {
        self.status
    }

    /// Start capture (cpal) — M1 logs only, no device opened so `cargo check` passes on headless WSL.
    pub fn start_capture(&mut self) {
        info!(
            sample_rate = self.config.sample_rate,
            frame_size = self.config.frame_size,
            "voice capture started (M1 skeleton — no device opened)"
        );
        self.status = VoiceStatus::Listening;
        // M3: init cpal InputStream, rubato resampler 44.1/48k → 16k, ort Silero VAD v5
        let _ = Self::ensure_audio_deps_linked();
    }

    pub fn stop_capture(&mut self) {
        info!("voice capture stopped");
        self.status = VoiceStatus::Idle;
    }

    /// Barge-in: hentikan playback speaker saat VAD trigger (FR-2).
    pub fn barge_in(&mut self) {
        if self.status == VoiceStatus::Speaking {
            warn!("barge-in triggered — clearing playback queue");
            self.status = VoiceStatus::Listening;
        }
    }

    /// Sentence-split aman untuk istilah teknis (PRD §14.2) — dipakai M3 untuk TTS per-kalimat.
    #[must_use]
    pub fn split_sentences(text: &str) -> Vec<String> {
        // Regex naif aman: hanya split jika [.!?] diikuti spasi/akhir-string.
        // Hindari split pada `main.rs`, `v1.0`, `127.0.0.1`, `cargo.lock`.
        let mut sentences = Vec::new();
        let mut start = 0;
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut i = 0;
        while i < n {
            let c = chars[i];
            if matches!(c, '.' | '!' | '?') {
                // cek: setelah tanda baca harus spasi atau akhir string
                let next_is_space_or_end = i + 1 >= n || chars[i + 1].is_whitespace();
                // cek: titik di dalam kata (file extension, IP, versi) → skip jika sebelum & sesudah alphanumeric
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
                    // skip spasi setelah tanda baca
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_split_technical_terms() {
        let text = "Check main.rs and PRD.md. Version v1.0 is ready! IP 127.0.0.1 ok?";
        let s = VoicePipeline::split_sentences(text);
        // main.rs / PRD.md / v1.0 / 127.0.0.1 tidak boleh jadi split point
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
        p.barge_in(); // no-op saat listening
        assert_eq!(p.status(), VoiceStatus::Listening);
        p.stop_capture();
        assert_eq!(p.status(), VoiceStatus::Idle);
    }
}
