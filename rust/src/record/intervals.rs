use super::*;
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

pub(super) struct FileOutput {
    pub path: PathBuf,
    pub writer: Option<encode::Writer>,
}

impl FileOutput {
    fn open(config: &Config, name: &str, id: u128) -> Result<Self, String> {
        Ok(Self {
            path: output_path(config, name, id),
            writer: Some(new_writer(config, name, id)?),
        })
    }
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        self.writer
            .as_mut()
            .unwrap()
            .write(samples)
            .map_err(|error| format!("{error}: {}", self.path.display()))
    }
    fn finish(&mut self) -> Result<(), String> {
        self.writer
            .take()
            .unwrap()
            .finish()
            .map_err(|error| format!("{error}: {}", self.path.display()))
    }
}

pub(super) struct DiskFiles {
    pub id: String,
    pub first: u64,
    pub files: Vec<Option<FileOutput>>,
    pub mixed: Option<FileOutput>,
    pub markers: markers::MarkerWriter,
    pub written: u64,
    pub map: Option<Timeline>,
}

impl DiskFiles {
    pub fn open(config: &Config, first: u64, id: u128) -> Result<Self, String> {
        if !config.separate && !config.mixed {
            return Err("Select at least one recording output".into());
        }
        std::fs::create_dir_all(&config.folder)
            .map_err(|e| format!("{}: {e}", config.folder.display()))?;
        let mixed = config
            .mixed
            .then(|| FileOutput::open(config, "Combined", id))
            .transpose()?;
        let files = config
            .inputs
            .iter()
            .enumerate()
            .map(|(i, (name, _, _))| {
                config
                    .separate
                    .then(|| FileOutput::open(config, &format!("{}-{}", name, i + 1), id))
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            id: id.to_string(),
            first,
            files,
            mixed,
            written: 0,
            map: None,
            markers: markers::MarkerWriter::new(
                config.folder.join(format!("Markers-{id}.labels.txt")),
                RATE as u32,
            )?,
        })
    }

    pub fn enable_map(&mut self, config: &Config, session: &str, frame: u64) -> Result<(), String> {
        if self.map.is_some() {
            return Ok(());
        }
        let base = config
            .folder
            .join(format!("Recording-{}.timeline.jsonl", self.id));
        let outputs = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, out)| {
                out.as_ref().map(
                    |out| json!({"file":out.path.file_name().and_then(|s|s.to_str()),"source":i}),
                )
            })
            .chain(self.mixed.iter().map(
                |out| json!({"file":out.path.file_name().and_then(|s|s.to_str()),"source":null}),
            ))
            .collect::<Vec<_>>();
        let mut timeline = Timeline::open(base).map_err(|e| format!("Recording timeline: {e}"))?;
        timeline.line(&json!({"v":1,"type":"recording","recording":self.id,"session":session,"start":self.first,
            "rate":RATE,"sources":config.inputs.iter().map(|(name,_,_)|name.as_str()).collect::<Vec<_>>(),"outputs":outputs,"format":config.format,"output_rate": if config.format.eq_ignore_ascii_case("wav") {config.rate} else {RATE as u32},
            "seek_precision":if config.format.eq_ignore_ascii_case("wav") {"pcm-frame"} else {"codec-dependent"},
            "mapped_from":frame,"saved_at_mapped_from": self.written}))?;
        self.map = Some(timeline);
        Ok(())
    }

    pub fn finalize(mut self, config: &Config, end: u64, healthy: bool) -> Result<(), String> {
        let mut failure = None;
        for output in self.files.iter_mut().flatten().chain(self.mixed.iter_mut()) {
            if let Err(error) = output.finish() {
                failure.get_or_insert(error);
            }
        }
        if let Some(mut timeline) = self.map.take() {
            let complete = healthy && failure.is_none();
            if complete {
                for output in self.files.iter().flatten().chain(self.mixed.iter()) {
                    let check = if config.format.eq_ignore_ascii_case("wav") {
                        timeline.wav_parts(&output.path, self.written, config.rate, config.channels)
                    } else {
                        timeline.identity(&output.path).map(|_| ())
                    };
                    if let Err(e) = check {
                        failure.get_or_insert(e);
                    }
                }
            }
            if let Err(e) = timeline.finish(end, self.written, complete && failure.is_none()) {
                failure.get_or_insert(e);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mapping {
    Saved,
    Pause,
    Silence,
}

struct Span {
    start: u64,
    end: u64,
    saved: u64,
    kind: Mapping,
}

pub(super) struct Timeline {
    partial: PathBuf,
    path: PathBuf,
    writer: BufWriter<File>,
    pending: Option<Span>,
    losses: Vec<Option<(u64, u64, &'static str)>>,
}

impl Timeline {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        let partial = path.with_extension("partial.jsonl");
        let writer = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)?,
        );
        Ok(Self {
            partial,
            path,
            writer,
            pending: None,
            losses: Vec::new(),
        })
    }
    fn line(&mut self, value: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.writer, value).map_err(|e| e.to_string())?;
        self.writer.write_all(b"\n").map_err(|e| e.to_string())
    }
    fn flush_span(&mut self) -> Result<(), String> {
        if let Some(span) = self.pending.take() {
            self.line(&json!({"type":"span","start":span.start,"end":span.end,
                "saved":span.saved,"kind":match span.kind {Mapping::Saved=>"saved",Mapping::Pause=>"pause",Mapping::Silence=>"silence"}}))?;
        }
        Ok(())
    }
    pub(super) fn span(
        &mut self,
        start: u64,
        end: u64,
        saved: u64,
        kind: Mapping,
    ) -> Result<(), String> {
        if let Some(last) = &mut self.pending
            && last.end == start
            && last.kind == kind
        {
            last.end = end;
            return Ok(());
        }
        self.flush_span()?;
        self.pending = Some(Span {
            start,
            end,
            saved,
            kind,
        });
        Ok(())
    }
    pub fn source_gap(
        &mut self,
        source: usize,
        start: u64,
        end: u64,
        cause: &'static str,
    ) -> Result<(), String> {
        if self.losses.len() <= source {
            self.losses.resize(source + 1, None);
        }
        if let Some((a, b, reason)) = self.losses[source] {
            if b == start && reason == cause {
                self.losses[source] = Some((a, end, cause));
                return Ok(());
            }
            self.line(
                &json!({"type":"source_gap","source":source,"start":a,"end":b,"cause":reason}),
            )?;
        }
        self.losses[source] = Some((start, end, cause));
        Ok(())
    }
    pub fn uncertain(&mut self, source: usize, at: u64) -> Result<(), String> {
        self.line(&json!({"type":"timing_uncertain","source":source,"observed_at":at}))
    }
    pub fn overlap(&mut self, id: &str, start: u64, end: u64) -> Result<(), String> {
        self.flush_span()?;
        self.line(&json!({"type":"transcript","interval":id,"start":start,"end":end}))
    }
    // Identify final files without reading hours of media at Stop.
    fn identity(&mut self, path: &Path) -> Result<u64, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
            return Err(format!(
                "{}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        let info = unsafe { info.assume_init() };
        let wide = |hi: u32, lo: u32| u64::from(hi) << 32 | u64::from(lo);
        let size = wide(info.nFileSizeHigh, info.nFileSizeLow);
        self.line(&json!({"type":"file_identity","file":path.file_name().and_then(|s|s.to_str()),"bytes":size,
            "last_write":wide(info.ftLastWriteTime.dwHighDateTime,info.ftLastWriteTime.dwLowDateTime),
            "volume":info.dwVolumeSerialNumber,"file_index":wide(info.nFileIndexHigh,info.nFileIndexLow)}))?;
        Ok(size)
    }
    fn wav_parts(
        &mut self,
        path: &Path,
        saved: u64,
        rate: u32,
        channels: u16,
    ) -> Result<(), String> {
        let expected = ((u128::from(saved) * u128::from(rate)) / u128::from(RATE)) as u64;
        let mut part = 1;
        let mut offset = 0;
        loop {
            let actual = if part == 1 {
                path.to_owned()
            } else {
                let mut stem = path.file_stem().unwrap().to_os_string();
                stem.push(format!("_part{part}.wav"));
                path.with_file_name(stem)
            };
            let size = self.identity(&actual)?;
            let count = size.saturating_sub(44) / (u64::from(channels) * 2);
            self.line(&json!({"type":"pcm_part","file":actual.file_name().and_then(|s|s.to_str()),"first_sample":offset,"samples":count,"rate":rate}))?;
            offset += count;
            if offset == expected {
                break;
            }
            if count == 0 || offset > expected {
                return Err("WAV output length differs from admitted frames".into());
            }
            part += 1;
        }
        Ok(())
    }
    fn finish(mut self, end: u64, saved: u64, complete: bool) -> Result<(), String> {
        for (index, span) in std::mem::take(&mut self.losses).into_iter().enumerate() {
            if let Some((a, b, reason)) = span {
                self.line(
                    &json!({"type":"source_gap","source":index,"start":a,"end":b,"cause":reason}),
                )?;
            }
        }
        self.flush_span()?;
        self.line(&json!({"type":"end","end":end,"saved_frames":saved,"complete":complete}))?;
        self.writer.flush().map_err(|e| e.to_string())?;
        self.writer
            .get_ref()
            .sync_data()
            .map_err(|e| e.to_string())?;
        drop(self.writer);
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        };
        // Do not replace an independent artifact with the same destination.
        if unsafe {
            MoveFileExW(
                wide(&self.partial).as_ptr(),
                wide(&self.path).as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(format!(
                "Cannot publish recording timeline: {}: {}",
                self.path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}
