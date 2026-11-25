use super::{guided, jobs};
use serde::Deserialize;
use serde_json::Value;
use std::fs::{File, symlink_metadata};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) struct Selection {
    pub(super) files: Vec<PathBuf>,
    pub(super) separate_clocks: bool,
    pub(super) duplicates: bool,
}

#[derive(Deserialize)]
struct Header {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    session: String,
    recording: String,
    outputs: Vec<Output>,
}
#[derive(Deserialize)]
struct Output {
    file: String,
    source: Option<usize>,
}

fn bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> Result<usize, String> {
    buf.clear();
    const LIMIT: usize = 128 * 1024;
    let count = reader
        .take(LIMIT as u64 + 1)
        .read_until(b'\n', buf)
        .map_err(|e| e.to_string())?;
    if count > LIMIT {
        return Err("Recording manifest record exceeds the size limit".into());
    }
    Ok(count)
}

fn manifest(path: &Path) -> Result<(String, bool, Vec<(PathBuf, bool)>), String> {
    let manifest = path.canonicalize().map_err(|e| e.to_string())?;
    let parent = manifest
        .parent()
        .ok_or("Recording manifest needs an output folder")?;
    let mut reader = BufReader::new(File::open(&manifest).map_err(|e| e.to_string())?);
    let mut buf = Vec::new();
    bounded_line(&mut reader, &mut buf)?;
    let header: Header =
        serde_json::from_slice(&buf).map_err(|_| "Invalid recording manifest header")?;
    if header.v != 1
        || header.kind != "recording"
        || header.session.is_empty()
        || header.recording.is_empty()
        || header.outputs.is_empty()
        || header.outputs.len() > 256
    {
        return Err("Unsupported recording manifest".into());
    }
    let mut parts: Vec<Vec<String>> = vec![Vec::new(); header.outputs.len()];
    let mut current = None;
    let mut next_sample = 0;
    let mut rate = 0;
    let mut total_parts = 0;
    let mut final_ok = false;
    while bounded_line(&mut reader, &mut buf)? != 0 {
        if final_ok {
            return Err("Unexpected record after the recording end".into());
        }
        let row: Value =
            serde_json::from_slice(&buf).map_err(|_| "Invalid recording manifest record")?;
        if row["type"] == "pcm_part" {
            let file = row["file"].as_str().ok_or("Missing PCM part filename")?;
            let first = row["first_sample"]
                .as_u64()
                .ok_or("Invalid PCM part offset")?;
            let count = row["samples"].as_u64().ok_or("Invalid PCM part length")?;
            let sample_rate = row["rate"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or("Invalid PCM part rate")?;
            if let Some(index) = header.outputs.iter().position(|o| o.file == file) {
                if !parts[index].is_empty() || first != 0 {
                    return Err("Repeated PCM root or unexpected first offset".into());
                }
                current = Some(index);
                next_sample = 0;
                rate = sample_rate;
            }
            let index = current.ok_or("Unexpected PCM part before its base file")?;
            if first != next_sample || rate != sample_rate {
                return Err("Discontinuous PCM part offsets or rates".into());
            }
            if !parts[index].is_empty() {
                let base = Path::new(&header.outputs[index].file)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("Invalid part filename")?;
                if file != format!("{base}_part{}.wav", parts[index].len() + 1) {
                    return Err("Unexpected PCM part filename or order".into());
                }
            }
            total_parts += 1;
            if total_parts > 10_000 {
                return Err("Too many PCM parts in one batch".into());
            }
            parts[index].push(file.to_owned());
            next_sample = first.checked_add(count).ok_or("PCM offset overflow")?;
        }
        if row["type"] == "end" {
            if row["complete"] != true {
                return Err("The recording did not finish successfully".into());
            }
            final_ok = true;
        }
    }
    if !final_ok {
        return Err("The recording is still open or lacks a final end record".into());
    }
    let mut files = Vec::new();
    let rolled = parts.iter().any(|files| files.len() > 1);
    for (output, mut names) in header.outputs.into_iter().zip(parts) {
        if names.is_empty() {
            names.push(output.file)
        }
        for filename in names {
            let name = Path::new(&filename);
            if name.file_name() != Some(name.as_os_str())
                || filename.contains([':', '/', '\\'])
                || !jobs::is_media(name)
            {
                return Err("Recording output must be a media file beside its manifest".into());
            }
            let path = parent.join(name);
            match symlink_metadata(&path) {
                Ok(meta) if !meta.is_file() || meta.file_attributes() & 0x400 != 0 => {
                    return Err("Recording output must be an ordinary file".into());
                }
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {} // Missing media gets an individual preflight result.
            }
            files.push((path, output.source.is_some()));
        }
    }
    Ok((header.recording, rolled, files))
}

pub(in crate::batch_queue) fn expand_paths(
    paths: Vec<PathBuf>,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Selection, String> {
    let mut files = Vec::new();
    let mut sessions = std::collections::HashSet::new();
    let mut plain = false;
    let mut duplicates = false;
    let mut any_rolled = false;
    for path in paths {
        if !path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".timeline.jsonl"))
        {
            plain = true;
            files.push(path);
            continue;
        }
        let (session, rolled, entries) = match manifest(&path) {
            Ok(entries) => entries,
            Err(reason) => {
                writeln!(output, "Skip {}: {reason}", path.display()).map_err(|e| e.to_string())?;
                continue;
            }
        };
        any_rolled |= rolled;
        sessions.insert(session);
        let mixed = entries.iter().any(|(_, stem)| *stem) && entries.iter().any(|(_, stem)| !*stem);
        let choice = if mixed {
            writeln!(
                output,
                "Session {} contains both a mix and separate tracks.",
                path.display()
            )
            .map_err(|e| e.to_string())?;
            write!(
                output,
                "Select stems, mix, or both (duplicate coverage) [stems]: "
            )
            .map_err(|e| e.to_string())?;
            output.flush().map_err(|e| e.to_string())?;
            let reply = guided::read_answer(input).map_err(|e| e.to_string())?;
            match reply.to_ascii_lowercase().as_str() {
                "" | "stems" => 1,
                "mix" => 2,
                "both" => {
                    duplicates = true;
                    3
                }
                _ => return Err("Choose stems, mix, or both.".into()),
            }
        } else {
            3
        };
        for (path, stem) in entries {
            if choice == 3 || (choice == 1 && stem) || (choice == 2 && !stem) {
                files.push(path)
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|p| seen.insert(p.clone()));
    if files.is_empty() {
        return Err("No eligible completed recordings were selected.".into());
    }
    Ok(Selection {
        files,
        separate_clocks: any_rolled || sessions.len() > 1 || (plain && !sessions.is_empty()),
        duplicates,
    })
}

pub(crate) fn expand(paths: Vec<PathBuf>) -> Result<(Vec<PathBuf>, bool, bool), String> {
    let selection = expand_paths(paths, &mut io::stdin().lock(), &mut io::stdout().lock())?;
    Ok((
        selection.files,
        selection.separate_clocks,
        selection.duplicates,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_completed_local_recording_expands_and_mixed_sources_require_a_choice() {
        let dir = std::env::temp_dir().join(crate::batch_queue::id("queue_manifest_test").unwrap());
        std::fs::create_dir(&dir).unwrap();
        let manifest_path = dir.join("Recording-test.timeline.jsonl");
        let header = r#"{"v":1,"type":"recording","session":"alpha","recording":"interval_a","outputs":[{"file":"mic.wav","source":0},{"file":"mix.wav","source":null}]}"#;
        std::fs::write(&manifest_path, header).unwrap();
        assert!(manifest(&manifest_path).is_err());
        for name in ["mic.wav", "mix.wav"] {
            std::fs::write(dir.join(name), b"local").unwrap();
        }
        std::fs::write(
            &manifest_path,
            format!("{header}\n{{\"type\":\"end\",\"complete\":true}}\n"),
        )
        .unwrap();
        let selection = expand_paths(
            vec![manifest_path.clone()],
            &mut &b"stems\n"[..],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(selection.files.len(), 1);
        assert_eq!(selection.files[0].file_name().unwrap(), "mic.wav");
        assert!(!selection.duplicates);
        std::fs::write(
            &manifest_path,
            format!(
                "{}\n{{\"type\":\"end\",\"complete\":true}}",
                header.replace("mic.wav", "../mic.wav")
            ),
        )
        .unwrap();
        assert!(manifest(&manifest_path).is_err());
        std::fs::write(dir.join("mic_part2.wav"), b"part2").unwrap();
        let parts = [
            serde_json::json!({"type":"pcm_part","file":"mic.wav","first_sample":0,"samples":10,"rate":16000}),
            serde_json::json!({"type":"pcm_part","file":"mic_part2.wav","first_sample":10,"samples":3,"rate":16000}),
            serde_json::json!({"type":"end","complete":true}),
        ];
        std::fs::write(
            &manifest_path,
            format!(
                "{header}\n{}",
                parts
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();
        let selected = expand_paths(
            vec![manifest_path.clone()],
            &mut &b"stems\n"[..],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            selected
                .files
                .iter()
                .map(|p| p.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["mic.wav", "mic_part2.wav"]
        );
        assert!(selected.separate_clocks);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
