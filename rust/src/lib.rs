pub mod capture;
pub mod encode;
mod flac_writer;
pub mod live_wire;
pub mod markers;
pub mod monitor;
mod mp3_writer;
mod opus_writer;
pub mod prefs;
pub mod recent_programs;
pub mod record;
pub mod sources;
pub mod transcription_config;
pub mod transcription_endpoint;

/// Input formats shown in the recorder's file picker and accepted by the worker.
pub const TRANSCRIPTION_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "flac", "m4a", "m4v", "mka", "mkv", "mov", "mp3", "mp4", "mpeg", "mpga", "oga",
    "ogg", "opus", "wav", "webm", "wma",
];
