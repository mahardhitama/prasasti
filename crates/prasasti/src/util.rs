use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSRange, NSString};

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

/// A notification-centre observation, removed when dropped.
pub struct Observer(Retained<ProtocolObject<dyn NSObjectProtocol>>);

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: the token came from this centre's addObserverForName:….
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self.0.as_ref()) };
    }
}

/// Runs `f` (guarded) on each `name` notification from `object` (any sender when None).
/// Only for notifications posted on the main thread, where the observers live.
pub fn observe(name: &str, object: Option<&AnyObject>, f: impl Fn() + 'static) -> Observer {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        guarded(&f);
    });
    // SAFETY: no queue means the block runs on the posting thread, the main one.
    Observer(unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(&NSString::from_str(name)),
            object,
            None,
            &block,
        )
    })
}
