//! Text read in windows: the encoding of a file is decided from its start, a
//! window of it is decoded on demand, and a position in the decoded text maps
//! back to the byte offset in the file, so a read can stop at any character and
//! a later call can continue at exactly that byte.
use std::io;

use crate::src::Src;
use crate::util::CP1252_HIGH;

/// The part of a file looked at to tell UTF-8 from Windows-1252.
const SNIFF_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc {
    Utf8,
    Cp1252,
    Utf16Le,
    Utf16Be,
}

/// How a file is encoded and how many bytes of byte-order mark it starts with.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    pub enc: Enc,
    pub bom: u64,
}

/// Whether `bytes` is UTF-8 that may only end in the middle of a character.
fn valid_utf8_prefix(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

/// BOM first; else UTF-8 when the first MiB is valid UTF-8, else Windows-1252.
pub fn sniff(src: &mut Src) -> io::Result<Stream> {
    let head = src.head(SNIFF_BYTES)?;
    let (enc, bom) = if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (Enc::Utf8, 3)
    } else if head.starts_with(&[0xFF, 0xFE]) {
        (Enc::Utf16Le, 2)
    } else if head.starts_with(&[0xFE, 0xFF]) {
        (Enc::Utf16Be, 2)
    } else if if head.len() as u64 >= src.len() {
        std::str::from_utf8(&head).is_ok()
    } else {
        valid_utf8_prefix(&head)
    } {
        (Enc::Utf8, 0)
    } else {
        (Enc::Cp1252, 0)
    };
    Ok(Stream { enc, bom })
}

/// A decoded stretch of a file with the means to find its byte offsets.
pub struct Window {
    pub text: String,
    enc: Enc,
    /// Invalid UTF-8 runs replaced by U+FFFD: (offset in `text`, bytes replaced).
    repl: Vec<(usize, usize)>,
    /// Whether the window ends at the end of the file.
    pub eof: bool,
}

impl Window {
    /// The byte offset, from the start of the window, of the character at `off` in the text.
    pub fn raw_of(&self, off: usize) -> u64 {
        let head = &self.text[..off];
        (match self.enc {
            Enc::Utf8 => {
                let shrunk: usize = self
                    .repl
                    .iter()
                    .take_while(|(at, _)| *at < off)
                    .map(|(_, n)| n)
                    .sum();
                let marks = self.repl.iter().take_while(|(at, _)| *at < off).count();
                off - marks * '\u{fffd}'.len_utf8() + shrunk
            }
            Enc::Cp1252 => head.chars().count(),
            Enc::Utf16Le | Enc::Utf16Be => head.encode_utf16().count() * 2,
        }) as u64
    }
}

fn decode_utf8(bytes: &[u8]) -> (String, Vec<(usize, usize)>) {
    let mut text = String::with_capacity(bytes.len());
    let mut repl = Vec::new();
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            repl.push((text.len(), chunk.invalid().len()));
            text.push('\u{fffd}');
        }
    }
    (text, repl)
}

fn decode_utf16(bytes: &[u8], le: bool) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|c| {
        if le {
            u16::from_le_bytes(*c)
        } else {
            u16::from_be_bytes(*c)
        }
    });
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{fffd}'))
        .collect()
}

/// Reads up to about `want` bytes at `pos` and decodes them, ending on a
/// character boundary unless the file ends.
pub fn read_window(src: &mut Src, st: Stream, pos: u64, want: usize) -> io::Result<Window> {
    let mut bytes = src.read_at(pos, want)?;
    let mut eof = pos + bytes.len() as u64 >= src.len();
    if !eof {
        match st.enc {
            Enc::Utf8 => {
                if let Err(e) = std::str::from_utf8(&bytes)
                    && e.error_len().is_none()
                {
                    bytes.truncate(e.valid_up_to());
                }
            }
            Enc::Cp1252 => {}
            Enc::Utf16Le | Enc::Utf16Be => {
                bytes.truncate(bytes.len() & !1);
                let last = bytes
                    .rchunks_exact(2)
                    .next()
                    .map(|c| {
                        if st.enc == Enc::Utf16Le {
                            u16::from_le_bytes([c[0], c[1]])
                        } else {
                            u16::from_be_bytes([c[0], c[1]])
                        }
                    })
                    .unwrap_or(0);
                if (0xD800..0xDC00).contains(&last) {
                    bytes.truncate(bytes.len() - 2);
                }
            }
        }
    } else {
        eof = true;
    }
    let (text, repl) = match st.enc {
        Enc::Utf8 => decode_utf8(&bytes),
        Enc::Cp1252 => (
            bytes
                .iter()
                .map(|&b| {
                    if (0x80..0xA0).contains(&b) {
                        CP1252_HIGH[usize::from(b - 0x80)]
                    } else {
                        char::from(b)
                    }
                })
                .collect(),
            Vec::new(),
        ),
        Enc::Utf16Le => (decode_utf16(&bytes, true), Vec::new()),
        Enc::Utf16Be => (decode_utf16(&bytes, false), Vec::new()),
    };
    Ok(Window {
        text,
        enc: st.enc,
        repl,
        eof,
    })
}

/// How many bytes to read so a window holds at least `room` bytes of JSON text.
pub fn window_bytes(enc: Enc, room: usize) -> usize {
    let per = if matches!(enc, Enc::Utf16Le | Enc::Utf16Be) {
        2
    } else {
        1
    };
    room * per + 64 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(bytes: &[u8]) -> (Src, Stream) {
        let mut s = Src::mem(bytes.to_vec());
        let st = sniff(&mut s).unwrap();
        (s, st)
    }

    #[test]
    fn the_encoding_comes_from_the_bom_then_the_content() {
        assert_eq!(stream(b"\xEF\xBB\xBFhi").1.enc, Enc::Utf8);
        assert_eq!(stream(b"\xEF\xBB\xBFhi").1.bom, 3);
        assert_eq!(stream(&[0xFF, 0xFE, b'h', 0]).1.enc, Enc::Utf16Le);
        assert_eq!(stream(&[0xFE, 0xFF, 0, b'h']).1.enc, Enc::Utf16Be);
        assert_eq!(stream("caf\u{e9}".as_bytes()).1.enc, Enc::Utf8);
        assert_eq!(stream(b"caf\xe9").1.enc, Enc::Cp1252);
    }

    #[test]
    fn offsets_map_back_through_every_encoding() {
        let (mut s, st) = stream("a\u{e9}\u{4e2d}b".as_bytes());
        let w = read_window(&mut s, st, 0, 100).unwrap();
        assert_eq!(w.raw_of(w.text.find('b').unwrap()), 6);
        let (mut s, st) = stream(b"caf\xe9!");
        let w = read_window(&mut s, st, 0, 100).unwrap();
        assert_eq!(w.text, "caf\u{e9}!");
        assert_eq!(w.raw_of(w.text.find('!').unwrap()), 4);
        let (mut s, st) = stream(&[0xFF, 0xFE, b'h', 0, 0x3d, 0xd8, 0x00, 0xde, b'!', 0]);
        let w = read_window(&mut s, st, 2, 100).unwrap();
        assert_eq!(w.raw_of(w.text.find('!').unwrap()), 6);
    }

    #[test]
    fn an_invalid_byte_in_utf8_is_replaced_and_its_offset_is_still_exact() {
        let mut bytes = "x".repeat(10).into_bytes();
        bytes.push(0xFF);
        bytes.extend_from_slice(b"end");
        let mut s = Src::mem(bytes);
        let st = Stream {
            enc: Enc::Utf8,
            bom: 0,
        };
        let w = read_window(&mut s, st, 0, 100).unwrap();
        assert!(w.text.contains('\u{fffd}'));
        assert_eq!(w.raw_of(w.text.find("end").unwrap()), 11);
    }

    #[test]
    fn a_window_never_ends_inside_a_character() {
        let data = "\u{4e2d}".repeat(10);
        let mut s = Src::mem(data.into_bytes());
        let st = Stream {
            enc: Enc::Utf8,
            bom: 0,
        };
        let w = read_window(&mut s, st, 0, 7).unwrap();
        assert_eq!(w.text.chars().count(), 2);
        assert!(!w.eof);
        let w = read_window(&mut s, st, 6, 100).unwrap();
        assert!(w.eof && w.text.chars().count() == 8);
    }
}
