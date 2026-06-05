//! flux-record — cinematic Claude Code session recorder.
//!
//! Pipeline:
//!   capture (x11grab+pulse) -> raw.mkv
//!   transcript.jsonl        -> Event stream
//!   render(raw + events)    -> cinematic.mp4 (filtergraph: ken-burns, vignette,
//!                              tool-call lower-third cards, HUD, karaoke captions)
//!   shorts(raw + events)    -> 9:16 vertical highlight clips
//!   chapters(events)        -> YouTube chapter timestamps

pub mod transcript;
pub mod ffmpeg;
pub mod capture;
pub mod overlay;
pub mod chapters;
pub mod shorts;
pub mod captions;
pub mod vite;
pub mod demo;
pub mod pty;
