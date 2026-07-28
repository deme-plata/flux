//! Pluggable frame sources.
//!
//! Three concrete sources ship, chosen so that **every path in this crate is
//! verifiable on the machine it was written on**:
//!
//! - [`SyntheticSource`] — generates a test pattern. No hardware needed, so the
//!   whole pipeline (encode → hash → stats → SAP → relay) is testable in CI and
//!   on a headless datacenter box like Epsilon, which has no camera at all.
//! - [`FileSource`] — serves a frame someone else dropped on disk (a phone
//!   upload, an `scp`, a shared mount).
//! - [`CommandSource`] — shells out to a real capture tool (`ffmpeg`,
//!   `fswebcam`, `libcamera-still`) on a box that *does* have a camera.
//!
//! **On the deliberate absence of a direct V4L2 backend:** hand-rolling ~400
//! lines of `unsafe` ioctl marshalling for `v4l2_format`/`v4l2_buffer` on a host
//! with no `/dev/video*` would produce code that could never be executed, let
//! alone tested, before shipping. That is the "built but not wired" failure mode
//! this codebase has been bitten by repeatedly. `CommandSource` reaches the same
//! hardware through a tool that is already correct, and can be swapped for a
//! native backend later without changing this trait.

use crate::frame::{Frame, FrameFormat};
use crate::png;
use std::fmt;
use std::path::{Path, PathBuf};

/// Errors a capture attempt can fail with.
#[derive(Debug)]
pub enum CaptureError {
    /// The underlying source could not produce anything.
    Unavailable(String),
    /// Bytes arrived but were not a recognisable image.
    Decode(String),
    /// The configured capture tool exited non-zero or was missing.
    Tool(String),
    Io(std::io::Error),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::Unavailable(m) => write!(f, "source unavailable: {m}"),
            CaptureError::Decode(m) => write!(f, "decode error: {m}"),
            CaptureError::Tool(m) => write!(f, "capture tool failed: {m}"),
            CaptureError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<std::io::Error> for CaptureError {
    fn from(e: std::io::Error) -> Self {
        CaptureError::Io(e)
    }
}

pub type CaptureResult = Result<Frame, CaptureError>;

/// Anything that can hand back a single frame on demand.
///
/// One-shot by construction: there is no `start()`/`stop()`, no background
/// thread and no subscription. A frame is produced only when someone calls
/// [`FrameSource::capture`]. That is a privacy property, not an oversight — a
/// camera wired into a production host must not be able to run on a timer.
pub trait FrameSource {
    /// Stable identifier recorded into `Frame::source`.
    fn name(&self) -> String;
    /// Grab exactly one frame.
    fn capture(&mut self) -> CaptureResult;
    /// Whether this source can currently produce anything (cheap probe).
    fn available(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Synthetic
// ---------------------------------------------------------------------------

/// A moving test pattern. Every call advances an internal counter so successive
/// frames differ — which is what makes it useful as a *liveness* fixture rather
/// than a constant: a relay that caches or replays frames is immediately visible
/// because the hash stops changing.
pub struct SyntheticSource {
    pub width: u32,
    pub height: u32,
    frame_no: u64,
}

impl SyntheticSource {
    pub fn new(width: u32, height: u32) -> Self {
        SyntheticSource { width: width.max(1), height: height.max(1), frame_no: 0 }
    }

    pub fn frame_no(&self) -> u64 {
        self.frame_no
    }

    /// Render the RGB buffer for the current counter value.
    fn render(&self) -> Vec<u8> {
        let w = self.width;
        let h = self.height;
        let phase = (self.frame_no % w as u64) as u32;
        let mut buf = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                // Diagonal gradient gives a stable, obviously-not-noise backdrop.
                let r = ((x * 255) / w) as u8;
                let g = ((y * 255) / h) as u8;
                // A vertical bar sweeps across so motion is unmistakable.
                let bar = if (x + phase) % w < (w / 24).max(2) { 255 } else { 40 };
                buf.push(r);
                buf.push(g);
                buf.push(bar as u8);
            }
        }
        buf
    }
}

impl FrameSource for SyntheticSource {
    fn name(&self) -> String {
        "synthetic".to_string()
    }

    fn capture(&mut self) -> CaptureResult {
        let rgb = self.render();
        let png_bytes = png::encode_rgb(self.width, self.height, &rgb)
            .ok_or_else(|| CaptureError::Decode("PNG encode rejected the buffer".into()))?;
        self.frame_no += 1;
        Ok(Frame::new(self.width, self.height, FrameFormat::Png, png_bytes, "synthetic"))
    }
}

// ---------------------------------------------------------------------------
// File drop
// ---------------------------------------------------------------------------

/// Serves whatever image currently sits at `path`.
pub struct FileSource {
    pub path: PathBuf,
}

impl FileSource {
    pub fn new(path: impl AsRef<Path>) -> Self {
        FileSource { path: path.as_ref().to_path_buf() }
    }
}

impl FrameSource for FileSource {
    fn name(&self) -> String {
        format!("file:{}", self.path.display())
    }

    fn available(&self) -> bool {
        self.path.exists()
    }

    fn capture(&mut self) -> CaptureResult {
        if !self.path.exists() {
            return Err(CaptureError::Unavailable(format!("{} does not exist", self.path.display())));
        }
        let bytes = std::fs::read(&self.path)?;
        let format = FrameFormat::sniff(&bytes)
            .ok_or_else(|| CaptureError::Decode("not a PNG or JPEG".into()))?;
        let (w, h) = probe_dimensions(&bytes, format).unwrap_or((0, 0));
        Ok(Frame::new(w, h, format, bytes, self.name()))
    }
}

// ---------------------------------------------------------------------------
// External capture command
// ---------------------------------------------------------------------------

/// Runs a real capture tool and picks up what it wrote.
///
/// Example (ffmpeg, V4L2 device, single frame):
/// ```text
/// CommandSource::new(
///     "ffmpeg",
///     ["-f","v4l2","-i","/dev/video0","-frames:v","1","-y","/tmp/frame.jpg"],
///     "/tmp/frame.jpg",
/// )
/// ```
pub struct CommandSource {
    pub program: String,
    pub args: Vec<String>,
    pub output: PathBuf,
}

impl CommandSource {
    pub fn new<I, S>(program: impl Into<String>, args: I, output: impl AsRef<Path>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        CommandSource {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            output: output.as_ref().to_path_buf(),
        }
    }

    /// The conventional ffmpeg one-shot for a V4L2 device.
    pub fn ffmpeg_v4l2(device: &str, output: impl AsRef<Path>) -> Self {
        let out = output.as_ref().to_path_buf();
        CommandSource::new(
            "ffmpeg",
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "v4l2",
                "-i",
                device,
                "-frames:v",
                "1",
                "-y",
                &out.to_string_lossy(),
            ],
            &out,
        )
    }
}

impl FrameSource for CommandSource {
    fn name(&self) -> String {
        format!("command:{}", self.program)
    }

    fn available(&self) -> bool {
        // Resolve the program on PATH without running it.
        std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths).any(|dir| dir.join(&self.program).is_file())
            })
            .unwrap_or(false)
    }

    fn capture(&mut self) -> CaptureResult {
        if !self.available() {
            return Err(CaptureError::Tool(format!("{} not found on PATH", self.program)));
        }
        // Remove any stale output first, so a silently-failing tool cannot make
        // us serve the previous frame as if it were fresh.
        let _ = std::fs::remove_file(&self.output);

        let status = std::process::Command::new(&self.program)
            .args(&self.args)
            .status()
            .map_err(|e| CaptureError::Tool(format!("{} failed to spawn: {e}", self.program)))?;
        if !status.success() {
            return Err(CaptureError::Tool(format!("{} exited with {status}", self.program)));
        }

        let bytes = std::fs::read(&self.output).map_err(|e| {
            CaptureError::Tool(format!("{} wrote no output to {}: {e}", self.program, self.output.display()))
        })?;
        let format = FrameFormat::sniff(&bytes)
            .ok_or_else(|| CaptureError::Decode("capture tool produced a non-image".into()))?;
        let (w, h) = probe_dimensions(&bytes, format).unwrap_or((0, 0));
        Ok(Frame::new(w, h, format, bytes, self.name()))
    }
}

// ---------------------------------------------------------------------------
// Dimension probing
// ---------------------------------------------------------------------------

/// Read width/height straight out of the container header.
///
/// PNG keeps them at a fixed offset in IHDR. JPEG requires walking the marker
/// segments to the first SOFn frame header, because there is no fixed location.
pub fn probe_dimensions(bytes: &[u8], format: FrameFormat) -> Option<(u32, u32)> {
    match format {
        FrameFormat::Png => {
            if bytes.len() < 24 {
                return None;
            }
            let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
            let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
            Some((w, h))
        }
        FrameFormat::Jpeg => {
            let mut i = 2; // skip SOI
            // `<=` not `<`: a SOF segment reads through byte i+8 inclusive, so
            // i+9 == len is still fully in bounds. Using `<` here dropped a SOF
            // that ended exactly at EOF — which is the common case for a
            // minimal/truncated JPEG header.
            while i + 9 <= bytes.len() {
                if bytes[i] != 0xFF {
                    i += 1;
                    continue;
                }
                let marker = bytes[i + 1];
                // SOF0..SOF15, excluding the non-frame markers DHT/JPG/DAC.
                let is_sof = (0xC0..=0xCF).contains(&marker)
                    && marker != 0xC4
                    && marker != 0xC8
                    && marker != 0xCC;
                if is_sof {
                    let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                    let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                    return Some((w, h));
                }
                let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
                if seg_len < 2 {
                    return None;
                }
                i += 2 + seg_len;
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_produces_a_decodable_png_of_the_right_size() {
        let mut s = SyntheticSource::new(64, 48);
        let f = s.capture().expect("synthetic capture must succeed");
        assert_eq!(f.format, FrameFormat::Png);
        assert!(f.verify());
        // The declared dimensions must match what is actually in the IHDR.
        assert_eq!(probe_dimensions(&f.data, FrameFormat::Png), Some((64, 48)));
    }

    #[test]
    fn synthetic_frames_actually_change() {
        let mut s = SyntheticSource::new(32, 32);
        let a = s.capture().unwrap();
        let b = s.capture().unwrap();
        assert_ne!(a.hash, b.hash, "the sweeping bar must move between frames");
        assert_eq!(s.frame_no(), 2);
    }

    #[test]
    fn file_source_reports_unavailable_rather_than_panicking() {
        let mut fs = FileSource::new("/definitely/not/here.png");
        assert!(!fs.available());
        assert!(matches!(fs.capture(), Err(CaptureError::Unavailable(_))));
    }

    #[test]
    fn file_source_round_trips_a_real_png() {
        let mut s = SyntheticSource::new(16, 16);
        let frame = s.capture().unwrap();
        let path = std::env::temp_dir().join("flux_webcam_roundtrip_test.png");
        std::fs::write(&path, &frame.data).unwrap();

        let mut fs = FileSource::new(&path);
        assert!(fs.available());
        let read_back = fs.capture().expect("should read the png back");
        assert_eq!(read_back.hash, frame.hash, "bytes must survive the round trip");
        assert_eq!((read_back.width, read_back.height), (16, 16));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_source_rejects_non_images() {
        let path = std::env::temp_dir().join("flux_webcam_not_an_image.png");
        std::fs::write(&path, b"this is plainly not a picture").unwrap();
        let mut fs = FileSource::new(&path);
        assert!(matches!(fs.capture(), Err(CaptureError::Decode(_))));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_capture_tool_is_reported_not_panicked() {
        let mut c = CommandSource::new("definitely-not-a-real-binary-xyz", ["--x"], "/tmp/none.jpg");
        assert!(!c.available());
        assert!(matches!(c.capture(), Err(CaptureError::Tool(_))));
    }

    #[test]
    fn jpeg_dimension_probe_walks_segments() {
        // SOI, then an APP0 segment we must skip, then SOF0 declaring 100x50.
        let jpeg: Vec<u8> = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, // APP0, length 4 (2 payload bytes)
            0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x32, 0x00, 0x64, // SOF0: h=0x32=50, w=0x64=100
        ];
        assert_eq!(probe_dimensions(&jpeg, FrameFormat::Jpeg), Some((100, 50)));
    }
}
