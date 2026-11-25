//! Point markers in Audacity's tab-separated label format.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// A lazily created sidecar whose positions use the recorded output timeline.
pub struct MarkerWriter {
    path: PathBuf,
    sample_rate: u32,
    file: Option<File>,
    failed: bool,
}

impl MarkerWriter {
    pub fn new(path: PathBuf, sample_rate: u32) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("Marker sample rate must be greater than zero".into());
        }
        Ok(Self {
            path,
            sample_rate,
            file: None,
            failed: false,
        })
    }

    /// Append a point at an output-frame offset and flush it before returning.
    pub fn mark(&mut self, output_frame: u64, label: &str) -> Result<(), String> {
        if self.failed {
            return Err(format!(
                "Cannot write marker to {} after a previous write failure",
                self.path.display()
            ));
        }
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&self.path)
                    .map_err(|error| {
                        format!("Cannot create marker file {}: {error}", self.path.display())
                    })?,
            );
        }

        // Use integer arithmetic to avoid losing precision for long sessions.
        let micros = (u128::from(output_frame) * 1_000_000 + u128::from(self.sample_rate) / 2)
            / u128::from(self.sample_rate);
        let seconds = micros / 1_000_000;
        let fraction = micros % 1_000_000;
        let text: String = label
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect();
        let timestamp = format!("{seconds}.{fraction:06}");
        let line = format!("{timestamp}\t{timestamp}\t{text}\n");
        let file = self.file.as_mut().expect("marker file was opened");
        if let Err(error) = file.write_all(line.as_bytes()).and_then(|()| file.flush()) {
            self.failed = true;
            return Err(format!(
                "Cannot write marker file {}: {error}",
                self.path.display()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "audio-capture-marker-{name}-{}-{timestamp}-{}.txt",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn writes_frame_aligned_point_labels_lazily() {
        let path = test_path("points");
        let mut markers = MarkerWriter::new(path.clone(), 48_000).unwrap();
        assert!(!path.exists());
        markers.mark(24_000, "first\tline\nsecond\r").unwrap();
        markers.mark(48_001, "Discord 🔊").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "0.500000\t0.500000\tfirst line second \n1.000021\t1.000021\tDiscord 🔊\n"
        );
    }

    #[test]
    fn preserves_existing_marker_file_on_collision() {
        let path = test_path("collision");
        fs::write(&path, b"existing").unwrap();
        let mut markers = MarkerWriter::new(path.clone(), 48_000).unwrap();
        assert!(markers.mark(0, "new").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"existing");
    }

    #[test]
    fn rejects_zero_rate_and_handles_long_frame_indices() {
        assert!(MarkerWriter::new(test_path("zero"), 0).is_err());
        let path = test_path("long");
        let mut markers = MarkerWriter::new(path.clone(), 48_000).unwrap();
        markers.mark(48_000 * 3600 * 100, "long").unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "360000.000000\t360000.000000\tlong\n"
        );
    }
}
