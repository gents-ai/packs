//! Byte-level helpers: text decoding, bounded zip access and archive paths.
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::rc::Rc;

/// A single archive entry may expand to at most this many bytes.
pub const MAX_ENTRY_BYTES: u64 = 96 * 1024 * 1024;
/// All entries read in one document may expand to at most this many bytes.
const MAX_TOTAL_BYTES: u64 = 768 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

/// Decodes text bytes: UTF-8 (BOM stripped), UTF-16 with a BOM, else Windows-1252.
pub fn decode_text(bytes: &[u8]) -> Cow<'_, str> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest);
    }
    let utf16 = |rest: &[u8], le: bool| -> String {
        let units = rest.as_chunks::<2>().0.iter().map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        });
        char::decode_utf16(units)
            .map(|r| r.unwrap_or('\u{fffd}'))
            .collect()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Cow::Owned(utf16(rest, true));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Cow::Owned(utf16(rest, false));
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(
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
        ),
    }
}

/// Percent-decodes an archive href.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (
                (b[i + 1] as char).to_digit(16),
                (b[i + 2] as char).to_digit(16),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolves `href` against the directory of `base` inside an archive: drops the
/// fragment and query, decodes percent escapes and folds `.` and `..`.
pub fn resolve(base: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or("");
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        base.rsplit_once('/')
            .map_or(Vec::new(), |(d, _)| d.split('/').collect())
    };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// A zip archive with bounded, byte-counted reads.
pub struct Zip<'a> {
    ar: zip::ZipArchive<Cursor<&'a [u8]>>,
    read: u64,
    shared: HashMap<String, Rc<[u8]>>,
    shared_bytes: usize,
}

/// Entries read through [`Zip::read_shared`] stay cached up to this many bytes
/// in total, so an image used on many pages is inflated once.
const SHARED_CACHE_BYTES: usize = 16 * 1024 * 1024;

impl<'a> Zip<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self, String> {
        let ar = zip::ZipArchive::new(Cursor::new(data))
            .map_err(|e| format!("not a readable zip container: {e}"))?;
        if ar.len() > MAX_ENTRIES {
            return Err(format!("the archive has more than {MAX_ENTRIES} entries"));
        }
        Ok(Self {
            ar,
            read: 0,
            shared: HashMap::new(),
            shared_bytes: 0,
        })
    }

    pub fn has(&mut self, name: &str) -> bool {
        self.ar.by_name(name).is_ok()
    }

    /// Reads one entry; `Ok(None)` when it does not exist.
    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let remaining = MAX_TOTAL_BYTES.saturating_sub(self.read);
        let mut file = match self.ar.by_name(name) {
            Ok(f) => f,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(e) => return Err(format!("cannot open {name} in the archive: {e}")),
        };
        let limit = MAX_ENTRY_BYTES.min(remaining);
        if file.size() > limit {
            return Err(format!(
                "{name} expands to {} bytes, over the {limit} byte limit",
                file.size()
            ));
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        (&mut file)
            .take(limit + 1)
            .read_to_end(&mut buf)
            .map_err(|e| format!("cannot decompress {name}: {e}"))?;
        if buf.len() as u64 > limit {
            return Err(format!("{name} expands past the {limit} byte limit"));
        }
        self.read += buf.len() as u64;
        Ok(Some(buf))
    }

    /// Like [`Zip::read`], but repeated reads of one entry (a logo used on every
    /// slide or chapter) return the cached bytes instead of inflating again.
    pub fn read_shared(&mut self, name: &str) -> Result<Option<Rc<[u8]>>, String> {
        if let Some(hit) = self.shared.get(name) {
            return Ok(Some(Rc::clone(hit)));
        }
        let Some(bytes) = self.read(name)? else {
            return Ok(None);
        };
        let bytes: Rc<[u8]> = bytes.into();
        if self.shared_bytes + bytes.len() <= SHARED_CACHE_BYTES {
            self.shared_bytes += bytes.len();
            self.shared.insert(name.to_string(), Rc::clone(&bytes));
        }
        Ok(Some(bytes))
    }

    pub fn read_text(&mut self, name: &str) -> Result<Option<String>, String> {
        Ok(self.read(name)?.map(|b| decode_text(&b).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_hrefs() {
        assert_eq!(
            resolve("OEBPS/text/ch1.xhtml", "../images/a%20b.png#x"),
            "OEBPS/images/a b.png"
        );
        assert_eq!(resolve("a.xhtml", "img/p.png"), "img/p.png");
        assert_eq!(resolve("x/y.xhtml", "/abs/p.png"), "abs/p.png");
        assert_eq!(resolve("x/y.xhtml", "../../../p.png"), "p.png");
    }

    #[test]
    fn decodes_utf8_utf16_and_cp1252() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi"), "hi");
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(
            decode_text(&[b'a', 0x93, b'b', 0x94, 0xE9]),
            "a\u{201c}b\u{201d}\u{e9}"
        );
    }

    #[test]
    fn a_shared_entry_is_inflated_once() {
        use std::io::Write;
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zw.start_file("logo.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(&[7u8; 1000]).unwrap();
        let data = zw.finish().unwrap().into_inner();
        let mut zip = Zip::open(&data).unwrap();
        let a = zip.read_shared("logo.png").unwrap().unwrap();
        let read_once = zip.read;
        let b = zip.read_shared("logo.png").unwrap().unwrap();
        assert!(Rc::ptr_eq(&a, &b));
        assert_eq!((read_once, zip.read), (1000, 1000));
        assert!(zip.read_shared("missing.png").unwrap().is_none());
    }

    #[test]
    fn percent_decoding_handles_edges() {
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%41%zz"), "A%zz");
    }
}
