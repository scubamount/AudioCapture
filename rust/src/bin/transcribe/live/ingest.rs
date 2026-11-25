//! Chunk ownership uses the original 48 kHz clock. Network audio never crosses a source or a hole.

pub(super) const RATE: usize = 48_000;
pub(super) const CHUNK: usize = 4 * RATE;
const PRE: usize = 160 * RATE / 1000;
const POST: usize = 240 * RATE / 1000;
#[derive(Debug)]
pub(super) enum Content {
    Audio(Vec<i16>),
    Quiet,
    Gap(&'static str),
}
#[derive(Debug)]
pub(super) struct Piece {
    pub source: usize,
    pub first: u64,
    pub end: u64,
    pub content: Content,
}

pub(super) struct Source {
    pub next: u64,
    index: usize,
    gate: bool,
    first: u64,
    samples: Vec<i16>,
    previous_tail_has_signal: bool,
    pending_zero: Option<u64>,
}

impl Source {
    pub fn new(index: usize, first: u64, gate: bool) -> Self {
        Self {
            index,
            next: first,
            gate,
            first,
            samples: Vec::new(),
            previous_tail_has_signal: false,
            pending_zero: None,
        }
    }

    fn flush(&mut self, out: &mut Vec<Piece>) {
        if self.samples.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut self.samples);
        if self.gate && samples.len() == CHUNK {
            let zero = samples.iter().all(|&s| s == 0);
            let hold = zero && !self.previous_tail_has_signal;
            self.previous_tail_has_signal =
                !zero && samples[CHUNK - POST..].iter().any(|&s| s != 0);
            if hold {
                // The entire core is exactly zero. Only its range needs to survive lookahead.
                self.pending_zero = Some(self.first);
                return;
            }
        }
        out.push(Piece {
            source: self.index,
            first: self.first,
            end: self.first + samples.len() as u64,
            content: Content::Audio(samples),
        });
    }

    fn resolve_zero(&mut self, end_of_run: bool, out: &mut Vec<Piece>) {
        if !end_of_run && self.samples.len() < PRE {
            return;
        }
        if let Some(first) = self.pending_zero.take() {
            let next = &self.samples[..self.samples.len().min(PRE)];
            let content = if next.iter().any(|&s| s != 0) {
                Content::Audio(vec![0; CHUNK])
            } else {
                Content::Quiet
            };
            out.push(Piece {
                source: self.index,
                first,
                end: first + CHUNK as u64,
                content,
            });
        }
    }

    fn append(&mut self, mut first: u64, mut samples: &[i16], out: &mut Vec<Piece>) {
        while !samples.is_empty() {
            if self.samples.is_empty() {
                self.first = first;
                self.samples.reserve_exact(CHUNK);
            }
            debug_assert_eq!(self.first + self.samples.len() as u64, first);
            let n = (CHUNK - self.samples.len()).min(samples.len());
            self.samples.extend_from_slice(&samples[..n]);
            if self.pending_zero.is_some() {
                self.resolve_zero(false, out);
            }
            if self.samples.len() == CHUNK {
                self.flush(out)
            }
            samples = &samples[n..];
            first += n as u64;
        }
    }

    pub fn audio(&mut self, first: u64, pcm: &[i16], out: &mut Vec<Piece>) -> Result<(), String> {
        let end = first
            .checked_add(pcm.len() as u64)
            .ok_or("Live packet clock overflow")?;
        if end <= self.next {
            return Ok(());
        }
        if first > self.next {
            self.gap(self.next, first, "missing source frames", out);
        }
        let skip = self.next.saturating_sub(first) as usize;
        self.append(first + skip as u64, &pcm[skip..], out);
        self.next = end;
        Ok(())
    }

    pub fn gap(&mut self, first: u64, end: u64, reason: &'static str, out: &mut Vec<Piece>) {
        if end <= self.next || end <= first {
            return;
        }
        self.seal(out);
        if self.next < first {
            out.push(Piece {
                source: self.index,
                first: self.next,
                end: first,
                content: Content::Gap("missing source frames"),
            });
        }
        out.push(Piece {
            source: self.index,
            first: self.next.max(first),
            end,
            content: Content::Gap(reason),
        });
        self.next = end;
    }

    pub fn seal(&mut self, out: &mut Vec<Piece>) {
        self.resolve_zero(true, out);
        self.flush(out);
        self.previous_tail_has_signal = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_loss_does_not_reclassify_an_earlier_unreported_hole() {
        let mut s = Source::new(0, 10, false);
        let mut out = Vec::new();
        s.gap(16, 20, "tap overflow", &mut out);
        assert!(matches!(
            &out[..],
            [
                Piece {
                    first: 10,
                    end: 16,
                    content: Content::Gap("missing source frames"),
                    ..
                },
                Piece {
                    first: 16,
                    end: 20,
                    content: Content::Gap("tap overflow"),
                    ..
                }
            ]
        ));
    }
    #[test]
    fn overlapping_packets_own_each_frame_once() {
        let mut s = Source::new(2, 10, false);
        let mut out = Vec::new();
        s.audio(10, &[21, 22, 23], &mut out).unwrap();
        s.audio(12, &[23, 24], &mut out).unwrap();
        s.audio(17, &[30], &mut out).unwrap();
        s.seal(&mut out);
        assert_eq!(
            out.iter().map(|x| (x.first, x.end)).collect::<Vec<_>>(),
            [(10, 14), (14, 17), (17, 18)]
        );
        assert!(matches!(&out[0].content, Content::Audio(p) if p==&[21,22,23,24]));
        assert!(matches!(out[1].content, Content::Gap(_)));
    }

    fn cores(pcm: &[i16], gate: bool) -> Vec<Piece> {
        let origin = 7;
        let mut src = Source::new(0, origin, gate);
        let mut result = Vec::new();
        let mut at = origin;
        for packet in pcm.chunks(777) {
            src.audio(at, packet, &mut result).unwrap();
            at += packet.len() as u64;
        }
        src.seal(&mut result);
        let mut cursor = origin;
        for piece in &result {
            assert_eq!(piece.first, cursor);
            let input = &pcm[(piece.first - origin) as usize..(piece.end - origin) as usize];
            match &piece.content {
                Content::Audio(wire) => assert_eq!(wire, input),
                Content::Quiet => {
                    assert!(gate && input.len() == CHUNK && input.iter().all(|&s| s == 0))
                }
                Content::Gap(_) => panic!("gap in generated input"),
            }
            cursor = piece.end;
        }
        assert_eq!(cursor, origin + pcm.len() as u64);
        result
    }

    #[test]
    fn whole_zero_core_obeys_both_guards_to_one_frame() {
        for (past, next, keep) in [
            (CHUNK - POST, PRE, true),
            (CHUNK - POST - 1, PRE - 1, true),
            (CHUNK - POST - 1, PRE, false),
            (CHUNK - 1, 0, true),
        ] {
            let mut pcm = vec![0; 3 * CHUNK + 17];
            pcm[past] = -1;
            pcm[2 * CHUNK + next] = 1;
            let pieces = cores(&pcm, true);
            assert_eq!(pieces.len(), 4);
            assert_eq!(matches!(pieces[1].content, Content::Audio(_)), keep);
            assert!(matches!(&pieces[3].content, Content::Audio(p) if p.len()==17));
            assert!(
                cores(&pcm, false)
                    .iter()
                    .all(|p| matches!(p.content, Content::Audio(_)))
            );
        }
    }

    #[test]
    fn consecutive_zero_cores_and_last_short_run_have_exact_coverage() {
        let mut pcm = vec![0; 4 * CHUNK + 17];
        pcm[4 * CHUNK + 16] = -1;
        let pieces = cores(&pcm, true);
        assert!(
            pieces[..3]
                .iter()
                .all(|p| matches!(p.content, Content::Quiet))
        );
        assert!(matches!(pieces[3].content, Content::Audio(_)));
        assert!(matches!(pieces[4].content, Content::Audio(_)));
        assert!(matches!(
            cores(&vec![0; CHUNK], true)[0].content,
            Content::Quiet
        ));
    }

    #[test]
    fn guard_never_crosses_a_source_gap() {
        let mut s = Source::new(0, 0, true);
        let mut out = Vec::new();
        let mut first = vec![0; CHUNK];
        first[CHUNK - 1] = 1;
        s.audio(0, &first, &mut out).unwrap();
        s.gap(CHUNK as u64, CHUNK as u64 + 100, "source lost", &mut out);
        s.audio(CHUNK as u64 + 100, &vec![0; CHUNK], &mut out)
            .unwrap();
        s.gap(
            (2 * CHUNK) as u64 + 100,
            (2 * CHUNK) as u64 + 200,
            "source lost",
            &mut out,
        );
        s.audio((2 * CHUNK) as u64 + 200, &[-1], &mut out).unwrap();
        s.seal(&mut out);
        assert!(matches!(
            &out[..],
            [
                Piece {
                    content: Content::Audio(_),
                    ..
                },
                Piece {
                    content: Content::Gap(_),
                    ..
                },
                Piece {
                    content: Content::Quiet,
                    ..
                },
                Piece {
                    content: Content::Gap(_),
                    ..
                },
                Piece {
                    content: Content::Audio(_),
                    ..
                }
            ]
        ));
        assert!(out.windows(2).all(|w| w[0].end == w[1].first));
    }
}
