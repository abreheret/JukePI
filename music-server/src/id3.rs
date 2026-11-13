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

/// The title to show on the amplifier for a track whose title is only its file name ("01. Come Together - The Beatles"):
/// the track number and the artist's name are dropped. A real tag title is returned as it is.
pub fn display_title(title: &str, artist: Option<&str>, file_stem: &str) -> String {
    if title.trim().to_lowercase() != file_stem.trim().to_lowercase() {
        return title.to_string(); // it came from a tag: leave it alone
    }
    let mut t = title.trim().to_string();
    // Leading number: "01. ", "02 - ", "3) ", "04_", or two digits and a space ("01 Come Together").
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits) {
        let rest = &t[digits..];
        let after_sep = rest.trim_start_matches([' ', '.', '-', '_', ')', '\u{2013}']);
        let has_sep = rest.starts_with(['.', '-', '_', ')', '\u{2013}']) || rest.starts_with(" -") || rest.starts_with(" .");
        let zero_padded = digits == 2 && t.starts_with('0') && rest.starts_with(' ');
        if !after_sep.is_empty() && after_sep.len() < rest.len() && (has_sep || zero_padded) {
            t = after_sep.to_string();
        }
    }
    if let Some(a) = artist.map(str::trim).filter(|a| !a.is_empty()) {
        let (lt, la) = (t.to_lowercase(), a.to_lowercase());
        if let Some(head) = lt.strip_suffix(&la).and_then(|h| h.trim_end().strip_suffix('-')).map(str::trim_end) {
            t = t[..head.len()].to_string();
        } else if let Some(tail) = lt.strip_prefix(&la).and_then(|h| h.trim_start().strip_prefix('-')).map(str::trim_start) {
            t = t[t.len() - tail.len()..].to_string();
        }
    }
    if t.contains('_') && !t.contains(' ') {
        t = t.replace('_', " ");
    }
    let t = t.trim().to_string();
    if t.is_empty() { title.to_string() } else { t }
}

/// The line the Denon's front panel scrolls: it only shows the title, so the artist goes into it ("Come Together -
/// The Beatles"). Nothing is added when the title already ends with the artist or the artist is unknown.
pub fn screen_title(title: &str, artist: Option<&str>) -> String {
    match artist.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) if !title.to_lowercase().trim_end().ends_with(&a.to_lowercase()) => format!("{title} - {a}"),
        _ => title.to_string(),
    }
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
    fn file_name_titles_are_cleaned_but_tag_titles_are_not() {
        let d = display_title;
        assert_eq!(d("01. Come Together - The Beatles", Some("The Beatles"), "01. Come Together - The Beatles"), "Come Together");
        assert_eq!(d("02 - Intro", Some("2Pac"), "02 - Intro"), "Intro");
        assert_eq!(d("3) Yesterday", None, "3) Yesterday"), "Yesterday");
        assert_eq!(d("04_Let_It_Be", None, "04_Let_It_Be"), "Let It Be");
        assert_eq!(d("01 Help", None, "01 Help"), "Help", "zero-padded number and a space");
        assert_eq!(d("The Beatles - Help", Some("The Beatles"), "The Beatles - Help"), "Help", "artist prefix");
        assert_eq!(d("21 Guns", None, "21 Guns"), "21 Guns", "a title that starts with a number");
        assert_eq!(d("1999", None, "1999"), "1999");
        assert_eq!(d("7", None, "7"), "7");
        assert_eq!(d("01. Come Together - The Beatles", Some("The Beatles"), "some other file name"), "01. Come Together - The Beatles", "a tag title is never touched");
        assert_eq!(d("01 - The Beatles", Some("The Beatles"), "01 - The Beatles"), "The Beatles", "a song called like its artist keeps its name");
        assert_eq!(d("01 -", None, "01 -"), "01 -", "nothing left after the number: keep the original");
    }

    #[test]
    fn the_artist_goes_into_the_scrolling_title() {
        assert_eq!(screen_title("Come Together", Some("The Beatles")), "Come Together - The Beatles");
        assert_eq!(screen_title("Come Together - The Beatles", Some("The Beatles")), "Come Together - The Beatles", "not twice");
        assert_eq!(screen_title("Intro", None), "Intro");
        assert_eq!(screen_title("Intro", Some("  ")), "Intro");
        assert_eq!(screen_title("Hey You", Some("Pink Floyd")), "Hey You - Pink Floyd");
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
