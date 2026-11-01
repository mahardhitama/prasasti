//! Image files that belong to a document: its `.assets` folder, and carrying
//! them into a new folder on Save / Save As.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// `<dir>/<stem>.assets` for a document at `doc_path`.
pub fn assets_dir(doc_path: &Path) -> PathBuf {
    let stem = doc_path.file_stem().unwrap_or_default().to_string_lossy();
    doc_path.with_file_name(format!("{stem}.assets"))
}

/// Creates `dir/name` (and `dir`), never overwriting: on a clash it tries
/// `name-2.ext`, `name-3.ext`…
pub fn create_new(dir: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    fs::create_dir_all(dir)?;
    let p = Path::new(name);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 1.. {
        let path = dir.join(if n == 1 { name.to_string() } else { format!("{stem}-{n}{ext}") });
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Copies each existing file in `paths` (absolute, UTF-16) that lies under one
/// of `sources` into `dest_dir`. Returns old path to copy, and the errors.
pub fn carry(paths: &[Vec<u16>], sources: &[PathBuf], dest_dir: &Path) -> (HashMap<Vec<u16>, Vec<u16>>, Vec<String>) {
    let (mut map, mut errors) = (HashMap::new(), Vec::new());
    for key in paths {
        let Ok(s) = String::from_utf16(key) else { continue };
        let src = Path::new(&s);
        if map.contains_key(key) || !sources.iter().any(|d| src.starts_with(d)) || !src.is_file() {
            continue;
        }
        let name = src.file_name().unwrap_or_default().to_string_lossy();
        let copied = create_new(dest_dir, &name).and_then(|(dest, mut f)| {
            let r = File::open(src).and_then(|mut from| io::copy(&mut from, &mut f));
            if r.is_err() {
                let _ = fs::remove_file(&dest); // our own partial copy
            }
            r.map(|_| dest)
        });
        match copied {
            Ok(dest) => {
                map.insert(key.clone(), dest.to_string_lossy().encode_utf16().collect());
            }
            Err(e) => errors.push(format!("Couldn't copy {name}: {e}")),
        }
    }
    (map, errors)
}
