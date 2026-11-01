use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

use objc2_foundation::{NSRange, NSString};

/// The string's UTF-16 code units, the unit every `core` range is measured in.
pub fn to_u16(s: &NSString) -> Vec<u16> {
    let len = s.length();
    let mut buf = vec![0u16; len];
    if len > 0 {
        // SAFETY: `buf` holds exactly `len` code units, matching the range.
        unsafe { s.getCharacters_range(NonNull::new_unchecked(buf.as_mut_ptr()), NSRange::new(0, len)) };
    }
    buf
}

/// Runs `f`, turning a panic into `None` so it never unwinds into Objective-C.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(e) => {
            let msg = e
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| e.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            eprintln!("prasasti: recovered from panic: {msg}");
            None
        }
    }
}
