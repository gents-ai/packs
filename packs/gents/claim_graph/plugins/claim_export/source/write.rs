//! Writing the rendered files under the bound folder. Each file is written to
//! a hidden temporary file of its own and renamed over its target, so a reader
//! never sees a half-written file and a failed call leaves the old one.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// A file to write: a path relative to the folder and its whole text.
pub struct Output {
    pub path: String,
    pub text: String,
}

pub fn write_all(dir: &Path, outputs: &[Output]) -> Result<(), String> {
    for output in outputs {
        write_one(dir, output)?;
    }
    Ok(())
}

fn write_one(dir: &Path, output: &Output) -> Result<(), String> {
    let target = dir.join(&output.path);
    let parent = target.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create the folder for {}: {e}", output.path))?;
    let (tmp, mut file) = temp_file(parent, &output.path)?;
    let written = file
        .write_all(output.text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", output.path))
        .and_then(|()| {
            std::fs::rename(&tmp, &target)
                .map_err(|e| format!("cannot replace {}: {e}", output.path))
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn temp_file(dir: &Path, what: &str) -> Result<(std::path::PathBuf, std::fs::File), String> {
    for n in 0..1000u32 {
        let path = dir.join(format!(".claim_export-{n}.part"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("cannot create a file for {what}: {e}")),
        }
    }
    Err(format!("cannot create a temporary file for {what}"))
}
