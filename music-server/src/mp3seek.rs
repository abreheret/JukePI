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

/// The Xing / Info / VBRI header frame some encoders (LAME, Lavf...) put first. It is not audio: it is a table for
/// seeking and the exact number of frames.
#[derive(Debug, PartialEq)]
pub struct HeaderFrame {
    /// Size of the frame in bytes (to skip it).
    pub len: u64,
    /// Exact length of the file, when the header says how many frames it holds.
    pub duration_ms: Option<u64>,
}

/// `Ok(None)`: the first frame after the ID3 tag is plain audio (or the file is not an MP3 we understand).
pub fn header_frame(path: &Path) -> io::Result<Option<HeaderFrame>> {
    let file = File::open(path)?;
    let mut r = BufReader::with_capacity(64 * 1024, file);
    let pos = id3v2_size(&mut r)?;
    r.seek(io::SeekFrom::Start(pos))?;
    let mut buf = [0u8; 192];
    let n = r.read(&mut buf)?;
    Ok(parse_header_frame(&buf[..n]))
}

fn be32(b: &[u8]) -> u64 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
}

fn parse_header_frame(buf: &[u8]) -> Option<HeaderFrame> {
    if buf.len() < 40 {
        return None;
    }
    let frame = parse_header([buf[0], buf[1], buf[2], buf[3]])?;
    // Xing / Info sit right after the side information (9 to 32 bytes, 2 more with a CRC); VBRI at a fixed place.
    let marker = buf[4..buf.len().min(4 + 40)].windows(4).position(|w| w == b"Xing" || w == b"Info").map(|p| p + 4);
    let frames = if let Some(m) = marker {
        // flags (4), then the number of frames (4) when bit 0 is set
        if buf.len() >= m + 12 && be32(&buf[m + 4..m + 8]) & 1 != 0 { Some(be32(&buf[m + 8..m + 12])) } else { None }
    } else if &buf[36..40] == b"VBRI" {
        if buf.len() >= 50 { Some(be32(&buf[46..50])) } else { None }
    } else {
        return None;
    };
    let duration_ms = frames.filter(|f| *f > 0).map(|f| (f as f64 * frame.duration_us / 1000.0).round() as u64);
    Some(HeaderFrame { len: frame.len, duration_ms })
}

/// Frames walked to decide whether a stream is constant bitrate and where to put the nudge.
const NUDGE_MAX_FRAMES: usize = 200;
/// The nudge goes after at least this many frames (about half a second): earlier, the Denon does not react.
const NUDGE_MIN_FRAMES: usize = 20;

/// The Denon shows the title of a stream only when it sees the frame headers change, which a constant-bitrate (CBR)
/// file never does (measured: 14 of 15 files, all the CBR ones showed `Url Stream`, all the VBR ones their title).
/// One 26 ms silent frame whose header differs (other bitrate AND the opposite CRC flag: a bitrate change alone did
/// not work) makes a CBR file show it. `audio` is the start of the audio (frame aligned).
/// Returns where to insert (a frame boundary) and the silent frame, or `None` for a stream too short or unreadable.
/// It is put in VBR streams too: one of them (the only one in the sample) showed `Url Stream` without it.
/// The frame goes where the next real frame does not reach back into the previous ones (its `main_data_begin` is 0),
/// so no sound is damaged.
pub fn bitrate_nudge(audio: &[u8]) -> Option<(usize, Vec<u8>)> {
    let mut pos = 0usize;
    let mut frames = 0usize;
    let mut choice: Option<(usize, [u8; 4])> = None;
    while frames < NUDGE_MAX_FRAMES && pos + 6 <= audio.len() {
        let h = [audio[pos], audio[pos + 1], audio[pos + 2], audio[pos + 3]];
        let f = parse_header(h)?;
        if choice.is_none() && frames >= NUDGE_MIN_FRAMES {
            // main_data_begin: first 9 bits of the side information (8 bits for MPEG 2 and 2.5), after the CRC if any
            let si = pos + 4 + if h[1] & 1 == 0 { 2 } else { 0 };
            let mdb = if (h[1] >> 3) & 3 == 3 { ((audio[si] as u16) << 1) | (audio[si + 1] as u16 >> 7) } else { audio[si] as u16 };
            if mdb == 0 {
                choice = Some((pos, h));
            }
        }
        pos += f.len as usize;
        frames += 1;
    }
    if frames < NUDGE_MIN_FRAMES + 2 {
        return None; // too short to bother
    }
    // No frame without back-reference found: put it after the minimum anyway (a glitch of one frame at worst).
    let (at, h) = match choice {
        Some(c) => c,
        None => {
            let mut p = 0usize;
            for _ in 0..NUDGE_MIN_FRAMES {
                p += parse_header([audio[p], audio[p + 1], audio[p + 2], audio[p + 3]])?.len as usize;
            }
            (p, [audio[p], audio[p + 1], audio[p + 2], audio[p + 3]])
        }
    };
    Some((at, silent_frame_like(h)?))
}

/// A Layer III frame of silence (all-zero payload) with the version, sample rate and channel mode of `h`, at another
/// bitrate (the lowest, or the next one when `h` already uses it), with the opposite protection (CRC) bit: a frame
/// whose header differs in two ways, which is what the Denon reacts to. No padding.
fn silent_frame_like(h: [u8; 4]) -> Option<Vec<u8>> {
    let version = (h[1] >> 3) & 3;
    let rate_index = ((h[2] >> 2) & 3) as usize;
    let current = h[2] >> 4;
    let index = if current == 1 { 2u8 } else { 1u8 };
    let (bitrate, sample_rate, coeff) = match version {
        3 => (BITRATES_V1_L3[index as usize], [44100u32, 48000, 32000].get(rate_index).copied()?, 144u64),
        2 => (BITRATES_V2_L3[index as usize], [22050u32, 24000, 16000].get(rate_index).copied()?, 72u64),
        0 => (BITRATES_V2_L3[index as usize], [11025u32, 12000, 8000].get(rate_index).copied()?, 72u64),
        _ => return None,
    };
    let len = (coeff * bitrate as u64 * 1000 / sample_rate as u64) as usize;
    let mut f = vec![0u8; len];
    f[..4].copy_from_slice(&[h[0], h[1] ^ 1, (index << 4) | (h[2] & 0x0D), h[3]]);
    Some(f)
}

/// Where the real audio of a file starts: after its ID3v2 tag and, with `skip_header_frame`, after the Xing / Info
/// header frame too. The Denon does not show the tags of a stream that starts with such a header frame (seen with a
/// Lavf-encoded file: `Url Stream`, while the same file without that frame shows them) and does not need it to play.
pub fn audio_start(path: &Path, skip_header_frame: bool) -> io::Result<u64> {
    let file = File::open(path)?;
    let mut r = BufReader::with_capacity(64 * 1024, file);
    let pos = id3v2_size(&mut r)?;
    if !skip_header_frame {
        return Ok(pos);
    }
    drop(r);
    Ok(match header_frame(path)? {
        Some(h) => pos + h.len,
        None => pos,
    })
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

    /// An Info frame: a normal frame header, then "Info", flags = frames present, `frames` frames.
    fn info_frame(frames: u32) -> Vec<u8> {
        let mut f = frame();
        f[36..40].copy_from_slice(b"Info");
        f[40..44].copy_from_slice(&1u32.to_be_bytes());
        f[44..48].copy_from_slice(&frames.to_be_bytes());
        f
    }

    /// `n` MPEG1 frames at `index` (bitrate index); frame `reaching` gets a non-zero main_data_begin.
    fn cbr_audio(n: usize, index: u8, reaching: &[usize]) -> Vec<u8> {
        let len = 144 * BITRATES_V1_L3[index as usize] as usize * 1000 / 44100;
        let mut out = vec![];
        for i in 0..n {
            let mut f = vec![0u8; len];
            f[..4].copy_from_slice(&[0xFF, 0xFB, index << 4, 0x44]); // MPEG1 L3, 44.1 kHz, no CRC, joint stereo
            if reaching.contains(&i) {
                f[4] = 0x20; // main_data_begin = 64: reaches into the previous frames
            }
            out.extend(f);
        }
        out
    }

    #[test]
    fn a_constant_bitrate_stream_gets_one_silent_frame_after_half_a_second() {
        let audio = cbr_audio(100, 9, &[]); // 128 kbps, 417 bytes
        let (at, frame) = bitrate_nudge(&audio).expect("CBR");
        assert_eq!(at, NUDGE_MIN_FRAMES * 417, "after 20 frames, on a frame boundary");
        assert_eq!(frame.len(), 144 * 32 * 1000 / 44100, "32 kbps");
        assert_eq!(&frame[..4], &[0xFF, 0xFA, 0x10, 0x44], "same version, rate and channel mode, other bitrate, opposite CRC flag");
        assert!(frame[4..].iter().all(|b| *b == 0), "silence");
        assert!(parse_header([frame[0], frame[1], frame[2], frame[3]]).is_some());
    }

    #[test]
    fn the_nudge_avoids_frames_that_reach_back_and_works_for_every_bitrate() {
        let audio = cbr_audio(100, 9, &[20, 21, 22]);
        let (at, _) = bitrate_nudge(&audio).unwrap();
        assert_eq!(at, 23 * 417, "frames 20 to 22 reach into the previous ones: it goes before frame 23");
        // frames that carry a CRC get a nudge without one
        let mut crc = cbr_audio(100, 9, &[]);
        for i in 0..100 {
            crc[i * 417 + 1] = 0xFA;
        }
        let (_, f) = bitrate_nudge(&crc).unwrap();
        assert_eq!(f[1], 0xFB);
        // a stream already at 32 kbps gets 40 kbps
        let (_, f32k) = bitrate_nudge(&cbr_audio(100, 1, &[])).unwrap();
        assert_eq!(f32k[2] >> 4, 2);
        // a VBR stream (two bitrates) gets it too
        let mut vbr = cbr_audio(30, 9, &[]);
        vbr.extend(cbr_audio(30, 11, &[]));
        assert_eq!(bitrate_nudge(&vbr).unwrap().0, 20 * 417);
        // too short, or not audio
        assert!(bitrate_nudge(&cbr_audio(10, 9, &[])).is_none());
        assert!(bitrate_nudge(b"not an mp3 at all, just text").is_none());
    }

    #[test]
    fn reads_the_exact_length_from_a_vbr_header() {
        let mut data = info_frame(1000);
        data.extend(frame());
        let t = tempfile_path::Tmp::new(&data);
        let h = header_frame(&t.0).unwrap().unwrap();
        assert_eq!(h.len, 417);
        assert_eq!(h.duration_ms, Some(26_122), "1000 frames of 26.122 ms");
        // No frame count in the header: still a header frame, but no exact length.
        let mut nofields = frame();
        nofields[36..40].copy_from_slice(b"Xing");
        let t = tempfile_path::Tmp::new(&nofields);
        assert_eq!(header_frame(&t.0).unwrap(), Some(HeaderFrame { len: 417, duration_ms: None }));
        // A plain audio frame is not a header.
        let plain = synthetic(0, 3);
        assert_eq!(header_frame(&plain.0).unwrap(), None);
    }

    #[test]
    fn skips_the_tag_and_optionally_the_header_frame() {
        let mut data = b"ID3\x03\x00\x00\x00\x00\x00\x5a".to_vec();
        data.resize(100, 0);
        data.extend(info_frame(3));
        for _ in 0..3 {
            data.extend(frame());
        }
        let t = tempfile_path::Tmp::new(&data);
        assert_eq!(audio_start(&t.0, true).unwrap(), 100 + 417, "after the tag and the Info frame");
        assert_eq!(audio_start(&t.0, false).unwrap(), 100, "only past the tag");
        let plain = synthetic(100, 3);
        assert_eq!(audio_start(&plain.0, true).unwrap(), 100, "no header frame: right after the tag");
        let bare = synthetic(0, 3);
        assert_eq!(audio_start(&bare.0, true).unwrap(), 0, "no tag at all");
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
