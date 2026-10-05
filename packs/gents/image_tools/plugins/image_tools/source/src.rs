//! Image sources and the folder they live in. A source is one file inside the
//! bound path, or one inline base64 image. Every relative name a caller gives
//! is checked here, once: no absolute path, no `..`, and no symbolic link below
//! the bound folder, so a call can neither read nor write outside it.
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::model::{Format, MAX_FILE_BYTES, MAX_INLINE_BASE64, hex};

/// Files a folder listing may hold before the caller must name the ones to read.
pub const MAX_FILES: usize = 10_000;
/// Deepest folder level a listing descends.
pub const MAX_DEPTH: usize = 16;

/// Anything an image decoder can read from.
pub trait Rs: Read + Seek {}
impl<T: Read + Seek> Rs for T {}

/// Where a source's bytes are.
#[derive(Clone, Debug)]
pub enum Data {
    /// A file inside the bound path.
    File(PathBuf),
    /// An inline image, shared between the steps that read it.
    Mem(Arc<Vec<u8>>),
    /// A name that was refused (it leaves the folder or goes through a link);
    /// opening it gives this sentence, so one bad name fails only its own record.
    Refused(String),
}

/// One image to work on.
#[derive(Clone, Debug)]
pub struct Source {
    /// The name shown in results: the path relative to the bound folder, or the inline name.
    pub name: String,
    /// The bytes.
    pub data: Data,
}

/// The sources of a call and the folder outputs may be written into.
pub struct Resolved {
    /// The sources, in the order they are processed.
    pub sources: Vec<Source>,
    /// The bound folder, when the call bound one; files are written below it.
    pub root: Option<PathBuf>,
    /// Files in a folder listing that are not images and were left out.
    pub skipped: usize,
}

impl Source {
    /// Opens the bytes for reading; the size cap is checked first.
    pub fn open(&self) -> Result<Box<dyn Rs + '_>, String> {
        match &self.data {
            Data::File(p) => {
                let f = File::open(p).map_err(|e| cannot_read(&self.name, &e))?;
                let len = f.metadata().map_err(|e| cannot_read(&self.name, &e))?.len();
                if len > MAX_FILE_BYTES {
                    return Err(format!(
                        "{} is over the {} MiB file limit; use a smaller file",
                        self.name,
                        MAX_FILE_BYTES >> 20
                    ));
                }
                Ok(Box::new(f))
            }
            Data::Mem(b) => Ok(Box::new(Cursor::new(b.as_slice()))),
            Data::Refused(why) => Err(why.clone()),
        }
    }

    /// Opens the bytes behind a buffer, the form the image decoders take.
    pub fn open_buf(&self) -> Result<std::io::BufReader<Box<dyn Rs + '_>>, String> {
        Ok(std::io::BufReader::with_capacity(1 << 16, self.open()?))
    }

    /// The size of the file in bytes.
    pub fn len(&self) -> Result<u64, String> {
        match &self.data {
            Data::File(p) => std::fs::metadata(p)
                .map(|m| m.len())
                .map_err(|e| cannot_read(&self.name, &e)),
            Data::Mem(b) => Ok(b.len() as u64),
            Data::Refused(why) => Err(why.clone()),
        }
    }

    /// SHA-256 of the file's bytes, streamed.
    pub fn sha256(&self) -> Result<String, String> {
        let mut r = self.open()?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = r.read(&mut buf).map_err(|e| cannot_read(&self.name, &e))?;
            if n == 0 {
                return Ok(hex(&h.finalize()));
            }
            h.update(&buf[..n]);
        }
    }

    /// The format from the first bytes, or `None` for anything that is not a supported image.
    pub fn sniff(&self) -> Result<Option<Format>, String> {
        let mut r = self.open()?;
        let mut head = [0u8; 16];
        let mut got = 0;
        while got < head.len() {
            match r.read(&mut head[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) => return Err(cannot_read(&self.name, &e)),
            }
        }
        Ok(Format::sniff(&head[..got]))
    }
}

fn cannot_read(name: &str, e: &std::io::Error) -> String {
    format!(
        "cannot read {name}: {}; check the name and that the path is bound for this call",
        why(e)
    )
}

/// A short reason for an I/O error, without the OS error number.
pub fn why(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        std::io::ErrorKind::AlreadyExists => "it already exists".into(),
        _ => {
            let s = e.to_string();
            s.split(" (os error").next().unwrap_or(&s).to_lowercase()
        }
    }
}

/// Decodes an inline base64 image of at most [`MAX_INLINE_BASE64`] characters.
pub fn inline(b64: &str, name: Option<&str>) -> Result<Source, String> {
    if b64.len() > MAX_INLINE_BASE64 {
        return Err(format!(
            "data_base64 is over the {} MiB limit; bind the file with path instead",
            MAX_INLINE_BASE64 >> 20
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| {
            "data_base64 is not valid base64; send the image bytes base64 encoded".to_string()
        })?;
    let name = name
        .and_then(|n| Path::new(n).file_name())
        .map_or_else(|| "inline".to_owned(), |n| n.to_string_lossy().into_owned());
    Ok(Source {
        name,
        data: Data::Mem(Arc::new(bytes)),
    })
}

/// Validates a name relative to the bound folder: not empty, not absolute, no
/// `..`, no NUL, no backslash. Returns it as a path of plain components.
pub fn safe_rel(name: &str) -> Result<PathBuf, String> {
    let bad = |why: &str| format!("{name:?} {why}; name a file inside the bound folder");
    if name.is_empty() {
        return Err("a file name is empty; name a file inside the bound folder".into());
    }
    if name.contains('\0') || name.contains('\\') {
        return Err(bad("holds a character that is not allowed"));
    }
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir => return Err(bad("leaves the bound folder")),
            _ => return Err(bad("is an absolute path")),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(bad("names no file"));
    }
    Ok(out)
}

/// Refuses a path below `root` that passes through a symbolic link.
pub fn no_links(root: &Path, rel: &Path) -> Result<(), String> {
    let mut p = root.to_path_buf();
    for c in rel.components() {
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symbolic link, which is not followed; name the file it points to",
                    rel.display()
                ));
            }
            Ok(_) => {}
            // A missing tail is fine here: the caller reports it when it opens the file.
            Err(_) => return Ok(()),
        }
    }
    Ok(())
}

/// A name in `files`, checked and joined to `root`, as a source. A name that
/// is refused still gives a source, which fails with the reason when opened.
pub fn named(root: &Path, name: &str) -> Source {
    let checked = safe_rel(name).and_then(|rel| no_links(root, &rel).map(|()| rel));
    match checked {
        Ok(rel) => Source {
            name: rel.to_string_lossy().into_owned(),
            data: Data::File(root.join(rel)),
        },
        Err(why) => Source {
            name: name.to_owned(),
            data: Data::Refused(why),
        },
    }
}

/// Resolves the call's sources from `path` (a file or a folder), `files` and inline data.
pub fn resolve(
    path: Option<&str>,
    files: &[String],
    data_base64: Option<&str>,
    name: Option<&str>,
) -> Result<Resolved, String> {
    if let Some(b64) = data_base64 {
        if path.is_some() || !files.is_empty() {
            return Err("give either path or data_base64, not both".into());
        }
        return Ok(Resolved {
            sources: vec![inline(b64, name)?],
            root: None,
            skipped: 0,
        });
    }
    let Some(path) = path else {
        return Err("give path (a file or folder) or data_base64 with an image".into());
    };
    let root = Path::new(path);
    let meta = std::fs::metadata(root).map_err(|e| {
        format!(
            "cannot read {path}: {}; the path must be bound for this call",
            why(&e)
        )
    })?;
    if meta.is_file() {
        if !files.is_empty() {
            return Err("file and files name images inside a folder, but path is a file".into());
        }
        let name = root
            .file_name()
            .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
        return Ok(Resolved {
            sources: vec![Source {
                name,
                data: Data::File(root.to_path_buf()),
            }],
            root: None,
            skipped: 0,
        });
    }
    let mut sources = Vec::new();
    let mut skipped = 0;
    if files.is_empty() {
        walk(root, root, 0, &mut sources, &mut skipped)?;
        if sources.is_empty() {
            return Err(format!(
                "{path} holds no PNG, JPEG, GIF, BMP, TIFF or WebP images"
            ));
        }
    } else {
        if files.len() > MAX_FILES {
            return Err(format!(
                "files lists more than {MAX_FILES} names; list fewer"
            ));
        }
        sources.extend(files.iter().map(|f| named(root, f)));
    }
    Ok(Resolved {
        sources,
        root: Some(root.to_path_buf()),
        skipped,
    })
}

/// Lists the images below `dir` in name order, skipping hidden entries and links.
fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<Source>,
    skipped: &mut usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot list {}: {}", dir.display(), why(&e)))?
        .flatten()
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            *skipped += 1;
            continue;
        }
        let p = e.path();
        if ft.is_dir() {
            walk(root, &p, depth + 1, out, skipped)?;
        } else if ft.is_file() {
            let name = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            let source = Source {
                name,
                data: Data::File(p),
            };
            if source.sniff().ok().flatten().is_none() {
                *skipped += 1;
                continue;
            }
            if out.len() >= MAX_FILES {
                return Err(format!(
                    "{} holds more than {MAX_FILES} images; list the ones to read in files",
                    root.display()
                ));
            }
            out.push(source);
        }
    }
    Ok(())
}

/// Writes `bytes` to `rel` below `root` without ever leaving a half-written file.
///
/// The bytes go to a temporary name in the same folder first. Without
/// `overwrite` the final name is created by a hard link, which fails if the
/// name exists, so an existing file is never replaced; with it the temporary
/// file is renamed over the target.
pub fn write_file(root: &Path, rel: &str, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    let rel = safe_rel(rel)?;
    no_links(root, &rel)?;
    let target = root.join(&rel);
    let shown = rel.display();
    if !overwrite && std::fs::symlink_metadata(&target).is_ok() {
        return Err(format!(
            "{shown} already exists; choose another name or set overwrite to true"
        ));
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create the folder for {shown}: {}; the call needs read-write access to the folder", why(&e)))?;
    }
    let mut tmp = target.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let denied = |e: std::io::Error| {
        format!(
            "cannot write {shown}: {}; the call needs read-write access to the folder",
            why(&e)
        )
    };
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(denied)?;
    if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(denied(e));
    }
    drop(f);
    let moved = if overwrite {
        std::fs::rename(&tmp, &target)
    } else {
        // A hard link fails when the name exists, so nothing is replaced even by a racing writer.
        // vertexia: a host that cannot hard link falls back to check-then-rename, which a racing writer can beat
        std::fs::hard_link(&tmp, &target)
            .and_then(|()| std::fs::remove_file(&tmp))
            .or_else(|e| match e.kind() {
                std::io::ErrorKind::Unsupported if std::fs::symlink_metadata(&target).is_err() => {
                    std::fs::rename(&tmp, &target)
                }
                std::io::ErrorKind::Unsupported => Err(std::io::ErrorKind::AlreadyExists.into()),
                _ => Err(e),
            })
    };
    moved.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!("{shown} already exists; choose another name or set overwrite to true")
        } else {
            denied(e)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("image_tools_src_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    #[test]
    fn safe_rel_accepts_plain_and_nested_names() {
        assert_eq!(safe_rel("a.png").unwrap(), PathBuf::from("a.png"));
        assert_eq!(safe_rel("sub/a.png").unwrap(), PathBuf::from("sub/a.png"));
        assert_eq!(
            safe_rel("./sub/./a.png").unwrap(),
            PathBuf::from("sub/a.png")
        );
    }

    #[test]
    fn safe_rel_refuses_every_escape() {
        for bad in [
            "",
            "..",
            "../a.png",
            "sub/../../a.png",
            "/etc/passwd",
            "a/../..",
            "a\\b.png",
            "a\0b",
            ".",
            "./",
        ] {
            let e = safe_rel(bad).unwrap_err();
            assert!(e.contains("bound folder"), "{bad:?}: {e}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_below_the_root_is_refused_for_reads_and_writes() {
        let d = dir("link");
        let outside = dir("link_outside");
        std::fs::write(outside.join("secret.png"), PNG).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.png"), d.join("l.png")).unwrap();
        std::os::unix::fs::symlink(&outside, d.join("ld")).unwrap();
        for name in ["l.png", "ld/secret.png"] {
            let e = named(&d, name).open().err().unwrap();
            assert!(e.contains("symbolic link"), "{name}: {e}");
        }
        let e = write_file(&d, "ld/new.png", PNG, true).unwrap_err();
        assert!(e.contains("symbolic link"), "{e}");
        assert!(!outside.join("new.png").exists());
        let e = write_file(&d, "l.png", PNG, true).unwrap_err();
        assert!(e.contains("symbolic link"), "{e}");
    }

    #[test]
    fn listing_returns_images_in_name_order_and_counts_the_rest() {
        let d = dir("list");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("b.png"), PNG).unwrap();
        std::fs::write(d.join("a.bin"), PNG).unwrap();
        std::fs::write(d.join("sub/c.dat"), PNG).unwrap();
        std::fs::write(d.join("notes.txt"), b"hello").unwrap();
        std::fs::write(d.join(".hidden.png"), PNG).unwrap();
        let r = resolve(d.to_str(), &[], None, None).unwrap();
        let names: Vec<_> = r.sources.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a.bin", "b.png", "sub/c.dat"]);
        assert_eq!(r.skipped, 1);
        assert_eq!(r.root.as_deref(), Some(d.as_path()));
    }

    #[test]
    fn an_empty_folder_and_a_missing_path_are_one_sentence() {
        let d = dir("empty");
        let e = resolve(d.to_str(), &[], None, None).err().unwrap();
        assert!(e.contains("holds no"), "{e}");
        let e = resolve(Some("/no/such/place"), &[], None, None)
            .err()
            .unwrap();
        assert!(e.contains("cannot read"), "{e}");
        let e = resolve(None, &[], None, None).err().unwrap();
        assert!(e.contains("path"), "{e}");
    }

    #[test]
    fn a_refused_name_is_a_source_that_fails_with_the_reason() {
        let d = dir("refused");
        for bad in ["../x.png", "/etc/passwd", "a/../../b"] {
            let s = named(&d, bad);
            assert_eq!(s.name, bad);
            let e = s.open().err().unwrap();
            assert!(e.contains("bound folder"), "{bad}: {e}");
            assert!(s.len().is_err() && s.sha256().is_err() && s.sniff().is_err());
        }
        let r = resolve(
            d.to_str(),
            &["../x.png".into(), "ok.png".into()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(r.sources.len(), 2, "one bad name does not drop the others");
    }

    #[test]
    fn a_file_path_is_one_source_and_refuses_files() {
        let d = dir("one");
        std::fs::write(d.join("a.png"), PNG).unwrap();
        let p = d.join("a.png");
        let r = resolve(p.to_str(), &[], None, None).unwrap();
        assert_eq!(r.sources.len(), 1);
        assert_eq!(r.sources[0].name, "a.png");
        assert!(r.root.is_none());
        assert!(resolve(p.to_str(), &["x".into()], None, None).is_err());
    }

    #[test]
    fn inline_data_is_decoded_named_and_bounded() {
        let s = inline("iVBORw0K", Some("dir/pic.png")).unwrap();
        assert_eq!(s.name, "pic.png");
        assert_eq!(s.len().unwrap(), 6);
        assert_eq!(inline("AAAA", None).unwrap().name, "inline");
        assert!(inline("***", None).err().unwrap().contains("base64"));
        let big = "A".repeat(MAX_INLINE_BASE64 + 4);
        assert!(inline(&big, None).err().unwrap().contains("limit"));
        assert!(resolve(Some("/x"), &[], Some("AAAA"), None).is_err());
    }

    #[test]
    fn write_is_atomic_and_never_clobbers_without_overwrite() {
        let d = dir("write");
        write_file(&d, "out/a.png", b"one", false).unwrap();
        assert_eq!(std::fs::read(d.join("out/a.png")).unwrap(), b"one");
        let e = write_file(&d, "out/a.png", b"two", false).unwrap_err();
        assert!(e.contains("already exists"), "{e}");
        assert_eq!(std::fs::read(d.join("out/a.png")).unwrap(), b"one");
        write_file(&d, "out/a.png", b"two", true).unwrap();
        assert_eq!(std::fs::read(d.join("out/a.png")).unwrap(), b"two");
        let left: Vec<_> = std::fs::read_dir(d.join("out"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(left.len(), 1, "no temporary file stays behind");
        assert!(write_file(&d, "../x.png", b"x", true).is_err());
        assert!(write_file(&d, "/tmp/x.png", b"x", true).is_err());
    }

    #[test]
    fn sniff_and_sha256_read_the_bytes() {
        let s = inline("iVBORw0KGgoAAAAA", None).unwrap();
        assert_eq!(s.sniff().unwrap(), Some(Format::Png));
        assert_eq!(s.sha256().unwrap().len(), 64);
        let t = inline("aGVsbG8=", None).unwrap();
        assert_eq!(t.sniff().unwrap(), None);
        assert_eq!(
            t.sha256().unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
