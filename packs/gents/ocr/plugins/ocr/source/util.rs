//! Byte-level helpers: text decoding, bounded zip access and archive paths.
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;

use crate::src::Src;

/// A single archive entry may expand to at most this many bytes.
pub const MAX_ENTRY_BYTES: u64 = 96 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

pub const CP1252_HIGH: [char; 32] = [
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

/// Reads and discards the first `n` bytes of a stream that cannot seek (a
/// deflated entry), to resume it where an earlier call stopped.
pub fn skip_bytes(rd: &mut impl Read, n: u64) -> Result<(), String> {
    let got = std::io::copy(&mut rd.take(n), &mut std::io::sink())
        .map_err(|e| format!("cannot read the document: {e}"))?;
    if got < n {
        return Err("the document is shorter than the position to resume at".into());
    }
    Ok(())
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

/// A zip archive read through seeks: the central directory is read once and
/// an entry is inflated only when asked for, whole (bounded) or as a stream.
pub struct Zip {
    ar: zip::ZipArchive<Src>,
    /// A second handle on the same bytes, for [`Zip::fork`].
    spare: Src,
    shared: HashMap<String, Rc<[u8]>>,
    shared_bytes: usize,
}

/// Entries read through [`Zip::read_shared`] stay cached up to this many bytes
/// in total, so an image used on many pages is inflated once.
const SHARED_CACHE_BYTES: usize = 16 * 1024 * 1024;
/// Entries parsed as one XML tree are at most this large: a tree takes several
/// times the size of its text, and larger parts are read as a stream.
pub const MAX_DOM_BYTES: u64 = 16 * 1024 * 1024;
/// An entry over [`RATIO_FLOOR`] bytes that expands more than this many times
/// its compressed size is refused: real XML and images stay far below it and a
/// zip bomb is far above.
const MAX_RATIO: u64 = 200;
const RATIO_FLOOR: u64 = 32 * 1024 * 1024;

fn expands_error(name: &str, size: u64, packed: u64) -> String {
    format!(
        "{name} expands to {size} bytes from {packed} compressed, which is over the {MAX_RATIO} to 1 limit for a safe archive"
    )
}

/// One archive entry as a byte stream that never yields more than it declared.
pub struct Entry<'a> {
    file: zip::read::ZipFile<'a, Src>,
    left: u64,
    pub size: u64,
}

impl Read for Entry<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = (buf.len() as u64).min(self.left + 1) as usize;
        let n = self.file.read(&mut buf[..want])?;
        if n as u64 > self.left {
            return Err(std::io::Error::other(
                "the entry expands past its declared size",
            ));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

impl Zip {
    pub fn open(src: Src) -> Result<Self, String> {
        let spare = src.reopen()?;
        let ar =
            zip::ZipArchive::new(src).map_err(|e| format!("not a readable zip container: {e}"))?;
        if ar.len() > MAX_ENTRIES {
            return Err(format!("the archive has more than {MAX_ENTRIES} entries"));
        }
        Ok(Self {
            ar,
            spare,
            shared: HashMap::new(),
            shared_bytes: 0,
        })
    }

    /// The same archive with its own read position, to stream one entry while
    /// this one serves lookups.
    pub fn fork(&self) -> Result<Self, String> {
        Self::open(self.spare.reopen()?)
    }

    pub fn has(&mut self, name: &str) -> bool {
        self.ar.by_name(name).is_ok()
    }

    /// Uncompressed size of an entry, `None` when it does not exist.
    pub fn size_of(&mut self, name: &str) -> Option<u64> {
        self.ar.by_name(name).ok().map(|f| f.size())
    }

    /// Streams one entry; `Ok(None)` when it does not exist.
    pub fn stream(&mut self, name: &str) -> Result<Option<Entry<'_>>, String> {
        let file = match self.ar.by_name(name) {
            Ok(f) => f,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(e) => return Err(format!("cannot open {name} in the archive: {e}")),
        };
        let (size, packed) = (file.size(), file.compressed_size());
        if size > RATIO_FLOOR && size / packed.max(1) > MAX_RATIO {
            return Err(expands_error(name, size, packed));
        }
        Ok(Some(Entry {
            left: size,
            size,
            file,
        }))
    }

    /// Reads one entry whole, at most `limit` bytes; `Ok(None)` when it does not exist.
    pub fn read_limited(&mut self, name: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
        let Some(mut entry) = self.stream(name)? else {
            return Ok(None);
        };
        if entry.size > limit {
            return Err(format!(
                "{name} expands to {} bytes, over the {limit} byte limit",
                entry.size
            ));
        }
        let mut buf = Vec::with_capacity(entry.size as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("cannot decompress {name}: {e}"))?;
        Ok(Some(buf))
    }

    /// Reads one entry whole (images and other parts); `Ok(None)` when it does not exist.
    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, String> {
        self.read_limited(name, MAX_ENTRY_BYTES)
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

    /// An XML part read whole for a tree parse, at most [`MAX_DOM_BYTES`].
    pub fn read_text(&mut self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .read_limited(name, MAX_DOM_BYTES)?
            .map(|b| decode_text(&b).into_owned()))
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
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zw.start_file("logo.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(&[7u8; 1000]).unwrap();
        let data = zw.finish().unwrap().into_inner();
        let mut zip = Zip::open(Src::mem(data)).unwrap();
        let a = zip.read_shared("logo.png").unwrap().unwrap();
        let b = zip.read_shared("logo.png").unwrap().unwrap();
        assert!(Rc::ptr_eq(&a, &b) && a.len() == 1000);
        assert!(zip.read_shared("missing.png").unwrap().is_none());
        assert_eq!(zip.size_of("logo.png"), Some(1000));
    }

    #[test]
    fn an_entry_streams_through_a_fork_while_the_original_serves_lookups() {
        use std::io::Write;
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("a.txt", opts).unwrap();
        zw.write_all(&[b'a'; 5000]).unwrap();
        zw.start_file("b.txt", opts).unwrap();
        zw.write_all(b"bee").unwrap();
        let data = zw.finish().unwrap().into_inner();
        let zip = Zip::open(Src::mem(data)).unwrap();
        let mut fork = zip.fork().unwrap();
        let mut zip = zip;
        let mut stream = fork.stream("a.txt").unwrap().unwrap();
        let mut first = [0u8; 10];
        stream.read_exact(&mut first).unwrap();
        assert_eq!(zip.read("b.txt").unwrap().unwrap(), b"bee");
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).unwrap();
        assert_eq!(rest.len(), 4990);
    }

    #[test]
    fn percent_decoding_handles_edges() {
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%41%zz"), "A%zz");
    }
}
