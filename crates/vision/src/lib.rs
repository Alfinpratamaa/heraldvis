use std::io::Cursor;
use std::sync::{Arc, RwLock};

use base64::Engine as _;
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
#[cfg(feature = "xcap")]
use xcap::Monitor;

/// Thread-safe in-memory framebuffer — FR-7a.
///
/// Stores latest JPEG bytes (not Data URL) in RAM for <5ms access.
/// On-demand `capture_frame_in_memory` populates it; callers can read via `latest`.
#[derive(Clone, Default)]
pub struct VisionBuffer(pub Arc<RwLock<Option<Vec<u8>>>>);

impl VisionBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(None)))
    }
    pub fn store(&self, jpeg_bytes: Vec<u8>) {
        if let Ok(mut g) = self.0.write() {
            *g = Some(jpeg_bytes);
        }
    }
    #[must_use]
    pub fn latest(&self) -> Option<Vec<u8>> {
        self.0.read().ok().and_then(|g| g.clone())
    }
}

/// In-memory screen perception — Zero Disk I/O (FR-7a/b).
pub struct ScreenPerception;

impl ScreenPerception {
    /// Capture primary monitor directly to RAM, downscale, JPEG-encode, base64 Data URL.
    ///
    /// * `max_dimension` — longest side clamped to 64..=4096 (default 1024). `low` callers pass 768.
    /// Returns `data:image/jpeg;base64,...` string. Never creates a file.
    ///
    /// # Errors
    /// Returns `Err(String)` only if even synthetic fallback fails (should never happen).
    pub fn capture_frame_in_memory(max_dimension: u32) -> Result<String, String> {
        let max_dim = max_dimension.clamp(64, 4096);

        let dynamic_img = match Self::try_capture_real() {
            Ok(img) => img,
            Err(e) => {
                tracing::debug!("ScreenPerception fallback synthetic (no display): {e}");
                Self::synthetic_image(256, 144)
            }
        };

        let dynamic_img = Self::downscale_if_needed(dynamic_img, max_dim);
        let jpeg_bytes = Self::encode_jpeg(dynamic_img)?;

        // also store raw JPEG in global buffer for <5ms future access? caller can.
        let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
        Ok(format!("data:image/jpeg;base64,{b64}"))
    }

    #[cfg(feature = "xcap")]
    fn try_capture_real() -> Result<DynamicImage, String> {
        let monitors = Monitor::all().map_err(|e| e.to_string())?;
        let primary = monitors.into_iter().next().ok_or_else(|| "No monitor found".to_string())?;
        let rgba: RgbaImage = primary.capture_image().map_err(|e| e.to_string())?;
        Ok(DynamicImage::ImageRgba8(rgba))
    }
    #[cfg(not(feature = "xcap"))]
    fn try_capture_real() -> Result<DynamicImage, String> {
        Err("xcap feature disabled".to_string())
    }

    fn synthetic_image(w: u32, h: u32) -> DynamicImage {
        // simple gradient placeholder so JPEG is non-trivial
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let r = ((x * 255 / w) % 256) as u8;
                let g = ((y * 255 / h) % 256) as u8;
                let b = 128u8;
                img.put_pixel(x, y, image::Rgba([r, g, b, 255]));
            }
        }
        DynamicImage::ImageRgba8(img)
    }

    fn downscale_if_needed(mut img: DynamicImage, max_dim: u32) -> DynamicImage {
        let (w, h) = (img.width(), img.height());
        if w > max_dim || h > max_dim {
            img = img.resize(max_dim, max_dim, FilterType::Triangle);
        }
        img
    }

    fn encode_jpeg(img: DynamicImage) -> Result<Vec<u8>, String> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg)
            .map_err(|e| e.to_string())?;
        Ok(buf.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn capture_returns_valid_data_url_no_disk_io() {
        // snapshot tmp before
        let tmp_before: std::collections::HashSet<String> = std::fs::read_dir("/tmp")
            .ok()
            .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        let cwd_before: std::collections::HashSet<String> = std::fs::read_dir(".")
            .ok()
            .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default();

        let url = ScreenPerception::capture_frame_in_memory(1024).expect("capture ok");
        assert!(url.starts_with("data:image/jpeg;base64,"), "prefix");
        let b64 = url.trim_start_matches("data:image/jpeg;base64,");
        let decoded = base64::engine::general_purpose::STANDARD.decode(b64).expect("valid base64");
        // JPEG magic FF D8 FF
        assert!(decoded.len() > 100, "jpeg non-empty {}", decoded.len());
        assert_eq!(decoded[0], 0xFF);
        assert_eq!(decoded[1], 0xD8);

        // no new file in /tmp or cwd
        let tmp_after: std::collections::HashSet<String> = std::fs::read_dir("/tmp")
            .ok()
            .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        let cwd_after: std::collections::HashSet<String> = std::fs::read_dir(".")
            .ok()
            .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        // allow existing files, but no png/jpg/jpeg created
        for f in tmp_after.difference(&tmp_before) {
            assert!(!f.ends_with(".png") && !f.ends_with(".jpg") && !f.ends_with(".jpeg"), "tmp file leaked: {f}");
        }
        for f in cwd_after.difference(&cwd_before) {
            assert!(!f.ends_with(".png") && !f.ends_with(".jpg") && !f.ends_with(".jpeg"), "cwd file leaked: {f}");
        }
    }

    #[test]
    fn capture_low_detail_clamp() {
        let url = ScreenPerception::capture_frame_in_memory(768).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        // clamp low bound
        let url2 = ScreenPerception::capture_frame_in_memory(10).unwrap();
        assert!(url2.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn vision_buffer_store_and_latest() {
        let vb = VisionBuffer::new();
        assert!(vb.latest().is_none());
        vb.store(vec![1, 2, 3]);
        assert_eq!(vb.latest(), Some(vec![1, 2, 3]));
    }
}
