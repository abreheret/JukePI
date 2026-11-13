//! Minimal ID3v2.3 tags for the amplifier.
//!
//! The Denon shows the title / artist / album it finds in the ID3 tag of the stream, and `Url Stream` when there is
//! none. Many files have no usable tag (a title only in the file name, an artist only in the folder name). The server
//! knows better, so the streams it hands to the amplifier start with a tag built from the index instead of the file's
//! own tag.

/// ID3 syncsafe integer: 4 bytes of 7 bits.
fn syncsafe(n: usize) -> [u8; 4] {
    [((n >> 21) & 0x7f) as u8, ((n >> 14) & 0x7f) as u8, ((n >> 7) & 0x7f) as u8, (n & 0x7f) as u8]
}

/// One text frame. ISO-8859-1 when every character fits, UTF-16 with a BOM otherwise.
fn text_frame(id: &str, text: &str) -> Vec<u8> {
    let mut body: Vec<u8> = vec![];
    if text.chars().all(|c| (c as u32) <= 0xFF) {
        body.push(0);
        body.extend(text.chars().map(|c| c as u8));
    } else {
        body.push(1);
        body.extend([0xFF, 0xFE]);
        for u in text.encode_utf16() {
            body.extend(u.to_le_bytes());
        }
    }
    let mut f = Vec::with_capacity(10 + body.len());
    f.extend(id.as_bytes());
    f.extend((body.len() as u32).to_be_bytes());
    f.extend([0, 0]);
    f.extend(body);
    f
}

/// A complete ID3v2.3 tag (title, artist, album; empty values are left out).
pub fn build_tag(title: &str, artist: Option<&str>, album: Option<&str>) -> Vec<u8> {
    let mut frames = text_frame("TIT2", title);
    if let Some(a) = artist.filter(|s| !s.is_empty()) {
        frames.extend(text_frame("TPE1", a));
    }
    if let Some(a) = album.filter(|s| !s.is_empty()) {
        frames.extend(text_frame("TALB", a));
    }
    let mut tag = vec![b'I', b'D', b'3', 3, 0, 0];
    tag.extend(syncsafe(frames.len()));
    tag.extend(frames);
    tag
}

/// Length in bytes of the ID3v2 tag at the start of `head` (10 bytes are enough), 0 when there is none.
pub fn existing_tag_len(head: &[u8]) -> u64 {
    if head.len() < 10 || &head[..3] != b"ID3" || head[6..10].iter().any(|b| b & 0x80 != 0) {
        return 0;
    }
    let size = ((head[6] as u64) << 21) | ((head[7] as u64) << 14) | ((head[8] as u64) << 7) | head[9] as u64;
    let footer = if head[3] >= 4 && head[5] & 0x10 != 0 { 10 } else { 0 };
    10 + size + footer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode our own tag back: (frame id, text).
    fn read_frames(tag: &[u8]) -> Vec<(String, String)> {
        assert_eq!(&tag[..3], b"ID3");
        assert_eq!(tag[3], 3);
        assert_eq!(existing_tag_len(tag) as usize, tag.len(), "declared size matches the real size");
        let mut pos = 10;
        let mut out = vec![];
        while pos < tag.len() {
            let id = String::from_utf8(tag[pos..pos + 4].to_vec()).unwrap();
            let n = u32::from_be_bytes(tag[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let body = &tag[pos + 10..pos + 10 + n];
            let text = if body[0] == 0 {
                body[1..].iter().map(|&b| b as char).collect()
            } else {
                assert_eq!(&body[1..3], &[0xFF, 0xFE]);
                let u: Vec<u16> = body[3..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16(&u).unwrap()
            };
            out.push((id, text));
            pos += 10 + n;
        }
        out
    }

    #[test]
    fn builds_a_readable_tag() {
        let t = build_tag("Come Together", Some("The Beatles"), Some("1 (One)"));
        assert_eq!(
            read_frames(&t),
            vec![("TIT2".into(), "Come Together".into()), ("TPE1".into(), "The Beatles".into()), ("TALB".into(), "1 (One)".into())]
        );
    }

    #[test]
    fn accents_stay_latin1_and_other_scripts_use_utf16() {
        let t = build_tag("Très haut", None, Some("Мир 平和"));
        assert_eq!(read_frames(&t), vec![("TIT2".into(), "Très haut".into()), ("TALB".into(), "Мир 平和".into())]);
        assert_eq!(t[10 + 10], 0, "the title is ISO-8859-1");
    }

    #[test]
    fn empty_artist_and_album_are_left_out() {
        let t = build_tag("Only a title", Some(""), None);
        assert_eq!(read_frames(&t).len(), 1);
    }

    #[test]
    fn finds_the_length_of_an_existing_tag() {
        assert_eq!(existing_tag_len(b"ID3\x04\x00\x00\x00\x00\x00\x17TS"), 10 + 23);
        assert_eq!(existing_tag_len(b"ID3\x04\x00\x10\x00\x00\x00\x17TS"), 10 + 23 + 10, "v2.4 footer");
        assert_eq!(existing_tag_len(b"\xff\xfb\x10\x00\x00\x00\x00\x00\x00\x00"), 0, "plain MP3 frame");
        assert_eq!(existing_tag_len(b"ID3"), 0, "too short");
        assert_eq!(existing_tag_len(b"ID3\x03\x00\x00\x80\x00\x00\x00"), 0, "not syncsafe: not a tag");
    }
}
