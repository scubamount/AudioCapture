//! Preserve MP4 track order. Windows can enumerate the last audio track first.
//! Hide later audio track headers in the reader's view while preserving every file offset.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;

pub(super) fn is_container(header: &[u8]) -> bool {
    matches!(
        header.get(4..8),
        Some(b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free" | b"skip")
    )
}

struct BoxHeader {
    kind: [u8; 4],
    type_offset: u64,
    payload: Range<u64>,
}

struct Scanner<'a> {
    file: &'a mut File,
    budget: u32,
}

impl Scanner<'_> {
    // Read only box headers and handler codes. Skip media payloads by seeking.
    fn next(&mut self, range: &mut Range<u64>) -> io::Result<Option<BoxHeader>> {
        if range.is_empty() {
            return Ok(None);
        }
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid MP4 box length");
        let start = range.start;
        let available = range.end - start;
        if available < 8 {
            return Err(invalid());
        }
        self.budget = self.budget.checked_sub(1).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "MP4 has too many metadata boxes",
            )
        })?;
        self.file.seek(SeekFrom::Start(start))?;
        let mut header = [0u8; 8];
        self.file.read_exact(&mut header)?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let (length, prefix) = match size {
            0 => (available, 8),
            1 if available >= 16 => {
                let mut extended = [0u8; 8];
                self.file.read_exact(&mut extended)?;
                (u64::from_be_bytes(extended), 16)
            }
            n => (u64::from(n), 8),
        };
        if length < prefix || length > available {
            return Err(invalid());
        }
        range.start = start + length;
        Ok(Some(BoxHeader {
            kind: header[4..].try_into().unwrap(),
            type_offset: start + 4,
            payload: start + prefix..range.start,
        }))
    }

    fn find(&mut self, mut range: Range<u64>, kind: &[u8; 4]) -> io::Result<Option<BoxHeader>> {
        while let Some(header) = self.next(&mut range)? {
            if header.kind == *kind {
                return Ok(Some(header));
            }
        }
        Ok(None)
    }

    fn audio_track(&mut self, track: Range<u64>) -> io::Result<bool> {
        let Some(media) = self.find(track, b"mdia")? else {
            return Ok(false);
        };
        let Some(handler) = self.find(media.payload, b"hdlr")? else {
            return Ok(false);
        };
        if handler.payload.end - handler.payload.start < 12 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid MP4 track handler",
            ));
        }
        self.file.seek(SeekFrom::Start(handler.payload.start + 8))?;
        let mut code = [0u8; 4];
        self.file.read_exact(&mut code)?;
        Ok(code == *b"soun")
    }
}

pub(super) fn additional_audio_tracks(file: &mut File) -> io::Result<Vec<u64>> {
    let length = file.metadata()?.len();
    let mut scan = Scanner {
        file,
        budget: 65_536,
    };
    let movie = scan
        .find(0..length, b"moov")?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "MP4 has no movie header"))?;
    let mut children = movie.payload;
    let mut found_audio = false;
    let mut hidden = Vec::new();
    while let Some(track) = scan.next(&mut children)? {
        if track.kind == *b"trak" && scan.audio_track(track.payload)? {
            if found_audio {
                hidden.push(track.type_offset);
            }
            found_audio = true;
        }
    }
    Ok(hidden)
}

pub(super) fn hide_tracks(bytes: &mut [u8], start: u64, offsets: &[u64]) {
    let end = start + bytes.len() as u64;
    let first = offsets.partition_point(|offset| *offset + 4 <= start);
    for &offset in &offsets[first..] {
        if offset >= end {
            break;
        }
        let from = offset.max(start);
        let to = (offset + 4).min(end);
        bytes[(from - start) as usize..(to - start) as usize]
            .copy_from_slice(&b"free"[(from - offset) as usize..(to - offset) as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_track_type_survives_partial_reads() {
        let mut bytes = *b"1234trak12345678trak";
        let offsets = [4, 16];
        for (i, piece) in bytes.chunks_mut(3).enumerate() {
            hide_tracks(piece, (i * 3) as u64, &offsets);
        }
        assert_eq!(bytes, *b"1234free12345678free");
    }
}
