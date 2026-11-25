//! Generate a self-contained review page for completed transcription results.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub struct ReviewTrack {
    pub label: String,
    pub audio: PathBuf,
    pub text: String,
    pub segments: Vec<ReviewSegment>,
    /// Chunk results without reliable timing, kept separate from timed text.
    pub untimed: Vec<String>,
}

pub struct ReviewSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Serialize)]
struct PageTrack<'a> {
    label: &'a str,
    audio: String,
    text: &'a str,
    segments: Vec<PageSegment<'a>>,
    untimed: &'a [String],
}

#[derive(Serialize)]
struct PageSegment<'a> {
    start: f64,
    end: f64,
    text: &'a str,
}

/// Convert a Windows absolute path into a file URL without losing reserved
/// characters. Windows canonical paths can use extended drive or UNC syntax.
fn windows_file_url(path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    let path = if let Some(unc) = path.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else if let Some(local) = path.strip_prefix("//?/") {
        local.to_owned()
    } else {
        path
    };
    let is_unc = path.starts_with("//");
    let drive = path.as_bytes().get(1) == Some(&b':')
        && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && path.as_bytes().get(2) == Some(&b'/');
    if !is_unc && !drive {
        return Err("Audio path is not an absolute Windows path".into());
    }
    // Extended device names are not browser file paths.
    if is_unc && (path.starts_with("//./") || path.starts_with("//?/")) {
        return Err("Unsupported Windows device path".into());
    }
    let mut result = if is_unc {
        "file:".to_owned()
    } else {
        "file:///".to_owned()
    };
    for (index, byte) in path.bytes().enumerate() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/')
            || (drive && index == 1 && byte == b':')
        {
            result.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(result, "%{byte:02X}").expect("writing into a string");
        }
    }
    Ok(result)
}

fn render_review(tracks: &[ReviewTrack], same_session: bool) -> Result<String, String> {
    let mut page_tracks = Vec::with_capacity(tracks.len());
    for track in tracks {
        let path = track.audio.canonicalize().map_err(|error| {
            format!(
                "Cannot open audio source {}: {error}",
                track.audio.display()
            )
        })?;
        if !path.is_file() {
            return Err(format!(
                "Audio source is not a file: {}",
                track.audio.display()
            ));
        }
        let path = path.to_str().ok_or("Audio path is not valid Unicode")?;
        let mut segments = Vec::with_capacity(track.segments.len());
        for segment in &track.segments {
            if !segment.start.is_finite()
                || !segment.end.is_finite()
                || segment.start < 0.0
                || segment.end < segment.start
            {
                return Err(format!("Invalid segment times for {}", track.label));
            }
            segments.push(PageSegment {
                start: segment.start,
                end: segment.end,
                text: &segment.text,
            });
        }
        page_tracks.push(PageTrack {
            label: &track.label,
            audio: windows_file_url(path)?,
            text: &track.text,
            segments,
            untimed: &track.untimed,
        });
    }
    let json = serde_json::to_string(&page_tracks)
        .map_err(|error| format!("Cannot prepare review: {error}"))?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    Ok(include_str!("review.html")
        .replace(
            "/* AUDIO_CAPTURE_SAME_SESSION */",
            if same_session { "true" } else { "false" },
        )
        .replace("/* AUDIO_CAPTURE_TRACKS */", &json))
}

/// Create the page without replacing an existing review or its source results.
pub fn write_review(
    output: &Path,
    tracks: &[ReviewTrack],
    same_session: bool,
) -> Result<(), String> {
    let page = render_review(tracks, same_session)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| format!("Cannot create review {}: {error}", output.display()))?;
    file.write_all(page.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|error| format!("Cannot write review {}: {error}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_escape_reserved_characters() {
        assert_eq!(
            windows_file_url(r"\\?\C:\Audio #1\é.opus").unwrap(),
            "file:///C:/Audio%20%231/%C3%A9.opus"
        );
        assert_eq!(
            windows_file_url(r"\\?\UNC\server\shared\a & b.wav").unwrap(),
            "file://server/shared/a%20%26%20b.wav"
        );
        assert!(windows_file_url(r"relative\track.opus").is_err());
        assert!(windows_file_url(r"\\.\PhysicalDrive0").is_err());
    }

    #[test]
    fn script_boundary_is_escaped_and_empty_segments_stay_untimed() {
        let path = std::env::current_exe().unwrap();
        let tracks = [ReviewTrack {
            label: "</script><script>alert(1)</script>".into(),
            audio: path,
            text: "untimed & untouched /* AUDIO_CAPTURE_SAME_SESSION */".into(),
            segments: vec![],
            untimed: vec![],
        }];
        let page = render_review(&tracks, false).unwrap();
        assert!(page.contains("\\u003c/script\\u003e"));
        assert!(!page.contains("</script><script>alert(1)</script>"));
        assert!(page.contains("\"segments\":[]"));
        assert!(page.contains("untouched /* AUDIO_CAPTURE_SAME_SESSION */"));
    }

    #[test]
    fn bad_times_are_rejected() {
        let track = ReviewTrack {
            label: "App".into(),
            audio: std::env::current_exe().unwrap(),
            text: String::new(),
            segments: vec![ReviewSegment {
                start: 2.0,
                end: 1.0,
                text: "bad".into(),
            }],
            untimed: vec![],
        };
        assert!(render_review(&[track], false).is_err());
    }

    #[test]
    fn partial_timing_keeps_timed_and_untimed_text() {
        let track = ReviewTrack {
            label: "App".into(),
            audio: std::env::current_exe().unwrap(),
            text: "whole result".into(),
            segments: vec![ReviewSegment {
                start: 1.0,
                end: 2.0,
                text: "timed phrase".into(),
            }],
            untimed: vec!["untimed phrase".into()],
        };
        let page = render_review(&[track], false).unwrap();
        assert!(page.contains("\"text\":\"timed phrase\""));
        assert!(page.contains("\"untimed\":[\"untimed phrase\"]"));
    }

    #[test]
    fn shared_timeline_is_an_explicit_choice() {
        let track = ReviewTrack {
            label: "App".into(),
            audio: std::env::current_exe().unwrap(),
            text: "text".into(),
            segments: vec![],
            untimed: vec![],
        };
        let separate = render_review(std::slice::from_ref(&track), false).unwrap();
        let shared = render_review(&[track], true).unwrap();
        assert!(separate.contains("const sameSession = false;"));
        assert!(shared.contains("const sameSession = true;"));
    }
}
