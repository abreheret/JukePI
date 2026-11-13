//! Find where an MP3 frame starting at (or just after) a given time begins, so the server can stream a file
//! "from 1:23". The Denon cannot seek inside a streamed URL (HEOS has no such command), so seeking is done by
//! handing it a URL whose content starts at the wanted frame.
//!
//! MP3 frames are independently decodable (apart from the bit reservoir: the very first frame after the cut may
//! glitch for a few milliseconds). Frames are walked one by one, which works for CBR and VBR alike, without
//! trusting a possibly missing Xing/VBRI table. Layer III only (all the library is).

use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use std::path::Path;

#[derive(Debug, PartialEq)]
pub struct SeekPoint {
    pub byte_offset: u64,
    /// Start time of that frame (>= the requested time, by at most one frame, ~26 ms), in milliseconds.
    pub actual_ms: u64,
}

struct Frame {
    len: u64,
    duration_us: f64,
}

const BITRATES_V1_L3: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const BITRATES_V2_L3: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];

/// Parse a 4 byte MPEG audio frame header (Layer III only).
fn parse_header(h: [u8; 4]) -> Option<Frame> {
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 0b11; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5, 1 = reserved
    let layer = (h[1] >> 1) & 0b11; // 1 = Layer III
    if version == 1 || layer != 1 {
        return None;
    }
    let bitrate_index = (h[2] >> 4) as usize;
    let rate_index = ((h[2] >> 2) & 0b11) as usize;
    let padding = ((h[2] >> 1) & 1) as u64;
    if bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
        return None; // free format / invalid
    }
    let (bitrate, sample_rate, samples) = match version {
        3 => (BITRATES_V1_L3[bitrate_index], [44100u32, 48000, 32000][rate_index], 1152u32),
        2 => (BITRATES_V2_L3[bitrate_index], [22050u32, 24000, 16000][rate_index], 576u32),
        _ => (BITRATES_V2_L3[bitrate_index], [11025u32, 12000, 8000][rate_index], 576u32),
    };
    let coeff = if version == 3 { 144u64 } else { 72u64 };
    let len = coeff * bitrate as u64 * 1000 / sample_rate as u64 + padding;
    Some(Frame { len, duration_us: samples as f64 * 1_000_000.0 / sample_rate as f64 })
}

/// Size of the ID3v2 tag at the start of the file (0 if none).
fn id3v2_size(r: &mut impl Read) -> io::Result<u64> {
    let mut h = [0u8; 10];
    if r.read_exact(&mut h).is_err() || &h[0..3] != b"ID3" {
        return Ok(0);
    }
    let size = ((h[6] as u64 & 0x7F) << 21) | ((h[7] as u64 & 0x7F) << 14) | ((h[8] as u64 & 0x7F) << 7) | (h[9] as u64 & 0x7F);
    let footer = if h[5] & 0x10 != 0 { 10 } else { 0 };
    Ok(10 + size + footer)
}

/// First frame whose start time is >= `target_ms`; the last frame if the file is shorter.
/// `Ok(None)`: no valid Layer III frame found (not an MP3 we can cut).
pub fn find_offset(path: &Path, target_ms: u64) -> io::Result<Option<SeekPoint>> {
    let file = File::open(path)?;
    let mut r = BufReader::with_capacity(256 * 1024, file);
    let mut pos = id3v2_size(&mut r)?;
    r.seek(io::SeekFrom::Start(pos))?;

    let target_us = target_ms as f64 * 1000.0;
    let mut elapsed_us = 0.0;
    let mut last: Option<SeekPoint> = None;
    let mut bad_bytes = 0u32;

    loop {
        let mut h = [0u8; 4];
        if r.read_exact(&mut h).is_err() {
            return Ok(last); // end of file
        }
        match parse_header(h) {
            Some(f) => {
                bad_bytes = 0;
                if elapsed_us >= target_us {
                    return Ok(Some(SeekPoint { byte_offset: pos, actual_ms: (elapsed_us / 1000.0) as u64 }));
                }
                last = Some(SeekPoint { byte_offset: pos, actual_ms: (elapsed_us / 1000.0) as u64 });
                elapsed_us += f.duration_us;
                r.seek_relative(f.len as i64 - 4)?;
                pos += f.len;
            }
            None => {
                // Not a frame start (junk, a tag in the middle, a damaged frame): resync byte by byte.
                bad_bytes += 1;
                if bad_bytes > 65_536 {
                    return Ok(last);
                }
                r.seek_relative(-3)?;
                pos += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// MPEG1 Layer III, 128 kbps, 44.1 kHz, no padding: 417 bytes and 26.122 ms per frame.
    fn frame() -> Vec<u8> {
        let mut f = vec![0xFF, 0xFB, 0x90, 0x00];
        f.resize(417, 0);
        f
    }

    fn synthetic(id3: usize, frames: usize) -> tempfile_path::Tmp {
        let mut data = Vec::new();
        if id3 > 0 {
            data.extend_from_slice(b"ID3\x03\x00\x00");
            let size = id3 - 10;
            data.extend_from_slice(&[((size >> 21) & 0x7F) as u8, ((size >> 14) & 0x7F) as u8, ((size >> 7) & 0x7F) as u8, (size & 0x7F) as u8]);
            data.resize(id3, 0);
        }
        for _ in 0..frames {
            data.extend_from_slice(&frame());
        }
        tempfile_path::Tmp::new(&data)
    }

    mod tempfile_path {
        use std::path::PathBuf;
        pub struct Tmp(pub PathBuf);
        impl Tmp {
            pub fn new(data: &[u8]) -> Tmp {
                let p = std::env::temp_dir().join(format!("mp3seek-{}-{:?}.mp3", std::process::id(), std::thread::current().id()));
                super::Write::write_all(&mut std::fs::File::create(&p).unwrap(), data).unwrap();
                Tmp(p)
            }
        }
        impl Drop for Tmp {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }

    #[test]
    fn header_parsing() {
        let f = parse_header([0xFF, 0xFB, 0x90, 0x00]).unwrap();
        assert_eq!(f.len, 417);
        assert!((f.duration_us - 26122.4).abs() < 1.0);
        assert!(parse_header([0x00, 0xFB, 0x90, 0x00]).is_none()); // no sync
        assert!(parse_header([0xFF, 0xFD, 0x90, 0x00]).is_none()); // Layer II
        assert!(parse_header([0xFF, 0xFB, 0xF0, 0x00]).is_none()); // bad bitrate
    }

    #[test]
    fn finds_frame_after_id3_tag() {
        let t = synthetic(100, 200); // ~5.2 s
        // frame k starts at k * 26.122 ms: the first one at or after 1000 ms is k = 39 (1018.8 ms)
        let p = find_offset(&t.0, 1000).unwrap().unwrap();
        assert_eq!(p.byte_offset, 100 + 39 * 417);
        assert_eq!(p.actual_ms, 1018);
    }

    #[test]
    fn zero_is_the_first_frame_and_past_the_end_is_the_last() {
        let t = synthetic(0, 50);
        assert_eq!(find_offset(&t.0, 0).unwrap().unwrap(), SeekPoint { byte_offset: 0, actual_ms: 0 });
        let end = find_offset(&t.0, 3_600_000).unwrap().unwrap();
        assert_eq!(end.byte_offset, 49 * 417);
    }

    #[test]
    fn resyncs_over_junk_and_rejects_non_mp3() {
        let mut data = vec![0x11u8; 33]; // junk before the first frame
        for _ in 0..40 {
            data.extend_from_slice(&frame());
        }
        let p = std::env::temp_dir().join(format!("mp3seek-junk-{}.mp3", std::process::id()));
        std::fs::File::create(&p).unwrap().write_all(&data).unwrap();
        let r = find_offset(&p, 500).unwrap().unwrap();
        assert_eq!((r.byte_offset - 33) % 417, 0, "must land on a frame boundary");
        std::fs::write(&p, vec![0x41u8; 10_000]).unwrap();
        assert!(find_offset(&p, 500).unwrap().is_none());
        let _ = std::fs::remove_file(&p);
    }
}
