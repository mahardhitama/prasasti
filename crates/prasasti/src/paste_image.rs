//! Image files that belong to a document: its `.assets` folder, and carrying
//! them into a new folder on Save / Save As.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use objc2::ClassType;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypeString, NSPasteboardURLReadingContentsConformToTypesKey,
    NSPasteboardURLReadingFileURLsOnlyKey,
};
use objc2_foundation::{NSArray, NSDate, NSDateFormatter, NSDictionary, NSHomeDirectory, NSLocale, NSNumber, NSString, NSURL, ns_string};
use prasasti_core::images;

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
/// Each copy is a new file, listed in the map's values.
pub fn carry(paths: &[Vec<u16>], sources: &[PathBuf], dest_dir: &Path) -> (HashMap<Vec<u16>, Vec<u16>>, Vec<String>) {
    let (mut map, mut errors) = (HashMap::new(), Vec::new());
    for key in paths {
        let Ok(s) = String::from_utf16(key) else { continue };
        let src = Path::new(&s);
        // `starts_with` is lexical: `assets/../x` must not count as inside `assets`.
        let inside = !src.components().any(|c| c == Component::ParentDir) && sources.iter().any(|d| src.starts_with(d));
        if map.contains_key(key) || !inside || !src.is_file() {
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

thread_local! {
    /// Tests only: replaces `pasted_root()`.
    pub static PASTED_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Where untitled documents' pasted images go, one `<uuid>` folder each.
pub fn pasted_root() -> PathBuf {
    PASTED_ROOT.with(|r| r.borrow().clone()).unwrap_or_else(|| {
        PathBuf::from(NSHomeDirectory().to_string()).join("Library/Application Support/Prasasti/Pasted")
    })
}

/// Deletes the folders in `root` not modified for `max_age`.
/// shortcut: by age only, a draft older than that loses its images; reference-check drafts if reported.
pub fn prune(root: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let old = e.metadata().ok().filter(|m| m.is_dir()).and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Markdown for the images on `pb`: image file URLs as links where
/// they are, else image data written to a new file. `Ok(None)`: no image.
/// `paste_dir` gives the untitled document's folder.
pub fn markdown_for_pasteboard(pb: &NSPasteboard, doc: Option<&Path>, paste_dir: &dyn Fn() -> PathBuf) -> Result<Option<String>, String> {
    let link = |alt: &str, path: &Path| {
        let abs: Vec<u16> = path.to_string_lossy().encode_utf16().collect();
        let dest = match doc.and_then(Path::parent) {
            Some(dir) => images::relative_path(&dir.to_string_lossy().encode_utf16().collect::<Vec<u16>>(), &abs),
            None => abs,
        };
        format!("![{alt}]({})", String::from_utf16_lossy(&images::format_destination(&dest, false)))
    };
    // SAFETY: AppKit's own constant keys.
    let keys = unsafe { [NSPasteboardURLReadingFileURLsOnlyKey, NSPasteboardURLReadingContentsConformToTypesKey] };
    let only_images = NSDictionary::from_slices(
        &keys,
        &[&*NSNumber::new_bool(true) as &AnyObject, &*NSArray::from_retained_slice(&[NSString::from_str("public.image")])],
    );
    // SAFETY: NSURL is a pasteboard-reading class; the options take these value types.
    let urls = unsafe { pb.readObjectsForClasses_options(&NSArray::from_slice(&[NSURL::class()]), Some(&only_images)) };
    let paths: Vec<PathBuf> = urls.iter().flat_map(|a| a.iter()).filter_map(|u| u.downcast::<NSURL>().ok()?.path()).map(|p| PathBuf::from(p.to_string())).collect();
    if !paths.is_empty() {
        let alt = |p: &Path| p.file_stem().unwrap_or_default().to_string_lossy().replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]");
        return Ok(Some(paths.iter().map(|p| link(&alt(p), p)).collect::<Vec<_>>().join("\n")));
    }
    // A non-image file comes with its icon as TIFF, and Office apps put a
    // rendering beside copied text: both paste as what they stand for.
    // SAFETY: AppKit's own constant.
    let has_text = pb.stringForType(unsafe { NSPasteboardTypeString }).is_some_and(|s| s.length() > 0);
    let has_file = pb.types().is_some_and(|t| t.containsObject(ns_string!("public.file-url")));
    if has_text || has_file {
        return Ok(None);
    }
    let types = ["public.png", "public.jpeg", "public.gif", "public.tiff", "public.heic"].map(NSString::from_str);
    let Some(kind) = pb.availableTypeFromArray(&NSArray::from_retained_slice(&types)) else { return Ok(None) };
    let Some(data) = pb.dataForType(&kind) else { return Ok(None) };
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't save pasted image: {e}");
    let (bytes, ext) = match kind.to_string().as_str() {
        "public.png" => (data.to_vec(), "png"),
        "public.jpeg" => (data.to_vec(), "jpg"),
        "public.gif" => (data.to_vec(), "gif"),
        _ => {
            let png = NSBitmapImageRep::imageRepWithData(&data)
                // SAFETY: an empty properties dictionary.
                .and_then(|rep| unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) })
                .ok_or_else(|| fail(&"unreadable image data"))?;
            (png.to_vec(), "png")
        }
    };
    let f = NSDateFormatter::new();
    f.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(ns_string!("en_US_POSIX"))));
    f.setDateFormat(Some(ns_string!("yyyyMMdd-HHmmss")));
    let name = format!("paste-{}.{ext}", f.stringFromDate(&NSDate::now()));
    let dir = doc.map_or_else(paste_dir, assets_dir);
    let (path, mut file) = create_new(&dir, &name).map_err(|e| fail(&e))?;
    if let Err(e) = file.write_all(&bytes) {
        let _ = fs::remove_file(&path); // our own partial file
        return Err(fail(&e));
    }
    Ok(Some(link("", &path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carry_rejects_parent_components() {
        let root = std::env::temp_dir().join(format!("prasasti-carry-dots-{}", std::process::id()));
        let src = root.join("doc.assets");
        fs::create_dir_all(&src).unwrap();
        fs::write(root.join("x.png"), b"x").unwrap();
        let sneaky: Vec<u16> = format!("{}/../x.png", src.display()).encode_utf16().collect();
        let (map, errors) = carry(&[sneaky], &[src], &root.join("out"));
        let _ = fs::remove_dir_all(&root);
        assert!(map.is_empty() && errors.is_empty());
    }
}
