//! Seekable input: a bound file read through its own handle (never loaded
//! whole) or the bytes of an inline file. Every reader in the plugin takes one
//! of these, so memory follows what a reader keeps, not the size of the file.
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::resume::Fnv;

/// Read buffer of a file handle; a seek inside it is free, one outside drops it.
const FILE_BUFFER: usize = 64 * 1024;

enum Inner {
    File(BufReader<File>),
    Mem(Cursor<Rc<[u8]>>),
}

/// A readable, seekable byte source with a known length.
pub struct Src {
    inner: Inner,
    len: u64,
    /// Where to open the same bytes again, for a second independent position.
    origin: Origin,
}

#[derive(Clone)]
enum Origin {
    Path(PathBuf),
    Mem(Rc<[u8]>),
}

impl Src {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("cannot read the file: {e}"))?;
        let meta = file
            .metadata()
            .map_err(|e| format!("cannot read the file: {e}"))?;
        if !meta.is_file() {
            return Err("not a file".into());
        }
        Ok(Self {
            len: meta.len(),
            inner: Inner::File(BufReader::with_capacity(FILE_BUFFER, file)),
            origin: Origin::Path(path.to_path_buf()),
        })
    }

    pub fn mem(bytes: Vec<u8>) -> Self {
        let shared: Rc<[u8]> = bytes.into();
        Self {
            len: shared.len() as u64,
            inner: Inner::Mem(Cursor::new(Rc::clone(&shared))),
            origin: Origin::Mem(shared),
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    /// The same bytes with their own read position, so one reader can stream
    /// an entry while another looks up a second one.
    pub fn reopen(&self) -> Result<Self, String> {
        match &self.origin {
            Origin::Path(p) => Self::open(p),
            Origin::Mem(m) => Ok(Self {
                len: self.len,
                inner: Inner::Mem(Cursor::new(Rc::clone(m))),
                origin: Origin::Mem(Rc::clone(m)),
            }),
        }
    }

    /// Up to `n` bytes from `offset`; shorter only at the end of the source.
    pub fn read_at(&mut self, offset: u64, n: usize) -> std::io::Result<Vec<u8>> {
        let n = n.min(self.len.saturating_sub(offset) as usize);
        let mut buf = vec![0u8; n];
        self.seek(SeekFrom::Start(offset))?;
        self.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// The first `n` bytes.
    pub fn head(&mut self, n: usize) -> std::io::Result<Vec<u8>> {
        self.read_at(0, n)
    }

    /// A 64-bit fingerprint of the length and the first and last 64 KiB, so a
    /// cursor can tell that the file it was issued for has changed.
    pub fn fingerprint(&mut self) -> std::io::Result<u64> {
        const SAMPLE: usize = 64 * 1024;
        let mut h = Fnv::new();
        h.field(&self.len.to_le_bytes());
        h.field(&self.read_at(0, SAMPLE)?);
        let tail = self.len.saturating_sub(SAMPLE as u64);
        h.field(&self.read_at(tail, SAMPLE)?);
        Ok(h.finish())
    }
}

impl Read for Src {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            Inner::File(f) => f.read(buf),
            Inner::Mem(m) => m.read(buf),
        }
    }
}

impl Seek for Src {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match &mut self.inner {
            Inner::File(f) => f.seek(pos),
            Inner::Mem(m) => m.seek(pos),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_at_offsets_and_reopens_with_its_own_position() {
        let mut a = Src::mem((0u8..=255).collect());
        assert_eq!(
            a.read_at(250, 100).unwrap(),
            vec![250, 251, 252, 253, 254, 255]
        );
        assert_eq!(a.read_at(300, 4).unwrap(), Vec::<u8>::new());
        let mut b = a.reopen().unwrap();
        a.seek(SeekFrom::Start(10)).unwrap();
        let mut one = [0u8; 1];
        b.read_exact(&mut one).unwrap();
        assert_eq!(one[0], 0);
        assert_eq!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
    }
}
