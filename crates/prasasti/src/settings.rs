//! The remembered editor font family and text zoom.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2_app_kit::{NSFont, NSFontManager, NSFontTraitMask, NSFontWeightRegular};
use objc2_foundation::{MainThreadMarker, NSNotificationCenter, NSString, NSUserDefaults};

/// Posted after the font or zoom changes; every open editor restyles.
pub const CHANGED: &str = "PrasastiSettingsChanged";

const FONT_KEY: &str = "editorFont";
const ZOOM_KEY: &str = "textZoom";
pub const DEFAULT_FAMILY: &str = "JetBrains Mono";
pub const BASE_SIZE: f64 = 15.0;

thread_local! {
    static OVERRIDE: RefCell<Option<Retained<NSUserDefaults>>> = const { RefCell::new(None) };
}

/// Replaces the user's defaults (tests pass a suite-named store).
pub fn set_defaults(d: Option<Retained<NSUserDefaults>>) {
    OVERRIDE.with(|o| *o.borrow_mut() = d);
}

fn defaults() -> Retained<NSUserDefaults> {
    OVERRIDE.with(|o| o.borrow().clone()).unwrap_or_else(NSUserDefaults::standardUserDefaults)
}

pub fn font_family() -> String {
    defaults()
        .stringForKey(&NSString::from_str(FONT_KEY))
        .map(|s| s.to_string())
        .unwrap_or_else(|| DEFAULT_FAMILY.into())
}

pub fn set_font_family(family: &str) {
    // SAFETY: a string is a property-list value.
    unsafe { defaults().setObject_forKey(Some(&NSString::from_str(family)), &NSString::from_str(FONT_KEY)) };
    notify();
}

pub fn zoom() -> f64 {
    let z = defaults().doubleForKey(&NSString::from_str(ZOOM_KEY));
    // 0 means unset; anything outside the ladder came from elsewhere.
    if (0.5..=3.0).contains(&z) { z } else { 1.0 }
}

pub fn set_zoom(zoom: f64) {
    defaults().setDouble_forKey(zoom, &NSString::from_str(ZOOM_KEY));
    notify();
}

fn notify() {
    // SAFETY: no object; observers ignore it.
    unsafe { NSNotificationCenter::defaultCenter().postNotificationName_object(&NSString::from_str(CHANGED), None) };
}

/// `family` at 15pt × `zoom`; JetBrains Mono if the family is missing, then
/// the system monospaced font (JetBrains Mono is only installed inside the bundle).
pub fn editor_font(family: &str, zoom: f64) -> Retained<NSFont> {
    let size = BASE_SIZE * zoom;
    let mtm = MainThreadMarker::new().expect("fonts are resolved on the main thread");
    let fm = NSFontManager::sharedFontManager(mtm);
    let find = |f: &NSString| fm.fontWithFamily_traits_weight_size(f, NSFontTraitMask::empty(), 5, size);
    find(&NSString::from_str(family))
        .or_else(|| find(&NSString::from_str(DEFAULT_FAMILY)))
        .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(size, unsafe { NSFontWeightRegular }))
}

/// The editor font from the current settings.
pub fn current_font() -> Retained<NSFont> {
    editor_font(&font_family(), zoom())
}
