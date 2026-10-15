//! The edit list of an MP4/M4A audio track (`moov/trak/edts/elst`), which
//! says how many encoder-priming frames to drop at the start and how long
//! the presented audio is.
//!
//! symphonia 0.5 (and 0.6) accept `FormatOptions { enable_gapless: true }`
//! but their MP4 reader ignores the edit list, so AAC priming (1024 to 2112
//! frames, 21 to 44 ms at 48 kHz) stays in the decoded audio while browsers
//! and ffmpeg drop it. The decoder applies [`Mp4Edit`] itself so word times
//! line up with the original file used for playback (research/08 pitfall 12).

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// The first audio track's edit, in that track's units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4Edit {
    /// `mdhd` timescale (units per second of `media_time` and `media_duration`).
    pub media_timescale: u32,
    /// `mdhd` duration: every encoded frame, priming and padding included.
    pub media_duration: u64,
    /// Media units skipped before presentation starts (the priming).
    pub skip: u64,
    /// Presented duration in media units, when the edit list says.
    pub present: Option<u64>,
}

impl Mp4Edit {
    /// `skip` in frames at `rate` Hz.
    pub fn skip_frames(&self, rate: u32) -> u64 {
        rescale(self.skip, self.media_timescale, rate)
    }

    /// Presented length in frames at `rate` Hz.
    pub fn present_frames(&self, rate: u32) -> Option<u64> {
        self.present.map(|p| rescale(p, self.media_timescale, rate))
    }

    /// Nothing to trim.
    pub fn is_identity(&self) -> bool {
        self.skip == 0 && self.present.is_none_or(|p| p >= self.media_duration)
    }
}

fn rescale(v: u64, from: u32, to: u32) -> u64 {
    if from == 0 || from == to {
        return v;
    }
    ((u128::from(v) * u128::from(to) + u128::from(from) / 2) / u128::from(from)) as u64
}

/// Reads the edit of the first `soun` track; `None` when the file is not an
/// MP4 or has no usable audio track.
pub fn audio_edit(path: &Path) -> Option<Mp4Edit> {
    let mut f = BufReader::new(File::open(path).ok()?);
    let len = f.seek(SeekFrom::End(0)).ok()?;
    f.seek(SeekFrom::Start(0)).ok()?;
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    if &head[4..8] != b"ftyp" {
        return None;
    }
    // Walk the top-level boxes to `moov` (it may come after `mdat`).
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let (size, kind, header) = box_header(&mut f, len - pos)?;
        if &kind == b"moov" {
            let body_len = usize::try_from(size - header).ok()?;
            if body_len > 256 << 20 {
                return None;
            }
            let mut body = vec![0u8; body_len];
            f.read_exact(&mut body).ok()?;
            return parse_moov(&body);
        }
        pos += size;
    }
    None
}

/// `(size, type, header length)` of the box at the reader's position;
/// `remaining` bounds a size-0 ("to end of file") box.
fn box_header(f: &mut impl Read, remaining: u64) -> Option<(u64, [u8; 4], u64)> {
    let mut h = [0u8; 8];
    f.read_exact(&mut h).ok()?;
    let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
    let kind = [h[4], h[5], h[6], h[7]];
    let (size, header) = match size32 {
        0 => (remaining, 8),
        1 => {
            let mut l = [0u8; 8];
            f.read_exact(&mut l).ok()?;
            (u64::from_be_bytes(l), 16)
        }
        n => (u64::from(n), 8),
    };
    (size >= header).then_some((size, kind, header))
}

/// Child boxes of a container body as `(type, body)`.
fn children(mut body: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while body.len() >= 8 {
        let size32 = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as u64;
        let kind = [body[4], body[5], body[6], body[7]];
        let (size, header) = match size32 {
            0 => (body.len() as u64, 8u64),
            1 if body.len() >= 16 => (
                u64::from_be_bytes(body[8..16].try_into().unwrap_or([0; 8])),
                16,
            ),
            1 => break,
            n => (n, 8),
        };
        if size < header || size > body.len() as u64 {
            break;
        }
        out.push((kind, &body[header as usize..size as usize]));
        body = &body[size as usize..];
    }
    out
}

fn child<'a>(body: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(body)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, b)| b)
}

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// `(timescale, duration)` of an `mvhd` or `mdhd` body (same layout up to there).
fn timescale_duration(b: &[u8]) -> Option<(u32, u64)> {
    match b.first()? {
        0 => Some((be_u32(b, 12)?, u64::from(be_u32(b, 16)?))),
        1 => Some((be_u32(b, 20)?, be_u64(b, 24)?)),
        _ => None,
    }
}

fn parse_moov(moov: &[u8]) -> Option<Mp4Edit> {
    let (movie_timescale, _) = timescale_duration(child(moov, b"mvhd")?)?;
    for (kind, trak) in children(moov) {
        if &kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else {
            continue;
        };
        let is_audio = child(mdia, b"hdlr").and_then(|h| h.get(8..12)) == Some(b"soun".as_slice());
        if !is_audio {
            continue;
        }
        let (media_timescale, media_duration) = timescale_duration(child(mdia, b"mdhd")?)?;
        let mut edit = Mp4Edit {
            media_timescale,
            media_duration,
            skip: 0,
            present: None,
        };
        if let Some(elst) = child(trak, b"edts").and_then(|e| child(e, b"elst")) {
            let version = *elst.first()?;
            let count = be_u32(elst, 4)? as usize;
            let entry = if version == 1 { 20 } else { 12 };
            let mut present_movie = 0u64;
            let mut first_media: Option<u64> = None;
            for i in 0..count.min(64) {
                let at = 8 + i * entry;
                let (segment, media_time) = if version == 1 {
                    (be_u64(elst, at)?, be_u64(elst, at + 8)? as i64)
                } else {
                    (
                        u64::from(be_u32(elst, at)?),
                        i64::from(be_u32(elst, at + 4)? as i32),
                    )
                };
                // media_time -1 is an empty edit (leading silence): ignored.
                if media_time < 0 {
                    continue;
                }
                first_media.get_or_insert(media_time as u64);
                present_movie += segment;
            }
            if let Some(skip) = first_media {
                edit.skip = skip;
                if present_movie > 0 {
                    edit.present = Some(rescale(present_movie, movie_timescale, media_timescale));
                }
            }
        }
        return Some(edit);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    /// `version/flags, creation, modification, timescale, duration` (v0).
    fn header_v0(timescale: u32, duration: u32) -> Vec<u8> {
        [0u32, 0, 0, timescale, duration]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect()
    }

    /// A `moov` body with one audio track and an optional one-entry edit list
    /// `(segment_duration in movie units, media_time)`.
    fn moov(movie_ts: u32, media_ts: u32, media_dur: u32, elst: Option<(u32, i32)>) -> Vec<u8> {
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 12]);
        let mdia = [
            bx(b"mdhd", &header_v0(media_ts, media_dur)),
            bx(b"hdlr", &hdlr),
        ]
        .concat();
        let mut trak = Vec::new();
        if let Some((segment, media_time)) = elst {
            let mut e = vec![0u8; 4];
            e.extend_from_slice(&1u32.to_be_bytes());
            e.extend_from_slice(&segment.to_be_bytes());
            e.extend_from_slice(&media_time.to_be_bytes());
            e.extend_from_slice(&[0, 1, 0, 0]);
            trak.extend(bx(b"edts", &bx(b"elst", &e)));
        }
        trak.extend(bx(b"mdia", &mdia));
        [bx(b"mvhd", &header_v0(movie_ts, 0)), bx(b"trak", &trak)].concat()
    }

    #[test]
    fn reads_priming_and_presented_length() {
        let edit = parse_moov(&moov(1_000, 48_000, 49_152, Some((1_000, 1_024)))).unwrap();
        assert_eq!(edit.skip_frames(48_000), 1_024);
        assert_eq!(edit.present_frames(48_000), Some(48_000));
        assert_eq!(edit.skip_frames(16_000), 341);
        assert!(!edit.is_identity());
        let none = parse_moov(&moov(48_000, 48_000, 96_000, None)).unwrap();
        assert!(none.is_identity());
        let zoom = parse_moov(&moov(48_000, 48_000, 96_256, Some((96_256, 0)))).unwrap();
        assert!(zoom.is_identity());
    }
}
