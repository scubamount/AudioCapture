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
    "aac", "aiff", "avi", "flac", "m4a", "m4v", "mka", "mkv", "mov", "mp3", "mp4", "mpeg", "mpg",
    "mpga", "oga", "ogg", "opus", "wav", "webm", "wma", "wmv",
];

/// Route these containers through local audio extraction before recognition.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "avi", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "webm", "wmv",
];

pub fn is_native_video(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|video| ext.eq_ignore_ascii_case(video))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn video_formats_are_selectable_and_case_insensitive() {
        for ext in VIDEO_EXTENSIONS {
            assert!(TRANSCRIPTION_EXTENSIONS.contains(ext));
            assert!(is_native_video(Path::new(&format!(
                "clip.{}",
                ext.to_uppercase()
            ))));
        }
        for file in ["clip.mp4.wav", "clip.m4a", "clip.opus", "clip"] {
            assert!(!is_native_video(Path::new(file)));
        }
    }
}
