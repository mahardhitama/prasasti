//! The remembered editor font family, text zoom, column width and appearance.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2_app_kit::{NSFont, NSFontManager, NSFontTraitMask, NSFontWeightRegular};
use objc2_foundation::{MainThreadMarker, NSNotificationCenter, NSString, NSUserDefaults};

/// Posted after the font, zoom or column width changes; every open editor restyles.
pub const CHANGED: &str = "PrasastiSettingsChanged";

const FONT_KEY: &str = "editorFont";
const ZOOM_KEY: &str = "textZoom";
const COLUMNS_KEY: &str = "columnWidth";
const APPEARANCE_KEY: &str = "appearance";
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

/// The editor's column, in characters (80 when unset).
pub fn column_width() -> usize {
    let key = NSString::from_str(COLUMNS_KEY);
    let d = defaults();
    if d.objectForKey(&key).is_none() { 80 } else { clamp_columns(d.integerForKey(&key)) }
}

pub fn set_column_width(n: isize) {
    defaults().setInteger_forKey(clamp_columns(n) as isize, &NSString::from_str(COLUMNS_KEY));
    notify();
}

/// "system", "light" or "dark" ("system" when unset or unknown).
pub fn appearance() -> &'static str {
    match defaults().stringForKey(&NSString::from_str(APPEARANCE_KEY)).map(|s| s.to_string()).as_deref() {
        Some("light") => "light",
        Some("dark") => "dark",
        _ => "system",
    }
}

pub fn set_appearance(mode: &str) {
    // SAFETY: a string is a property-list value.
    unsafe { defaults().setObject_forKey(Some(&NSString::from_str(mode)), &NSString::from_str(APPEARANCE_KEY)) };
    notify();
}

fn clamp_columns(n: isize) -> usize {
    n.clamp(40, 200) as usize
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

#[cfg(test)]
mod tests {
    use super::clamp_columns;

    #[test]
    fn clamps_columns() {
        assert_eq!(clamp_columns(10), 40);
        assert_eq!(clamp_columns(80), 80);
        assert_eq!(clamp_columns(999), 200);
    }
}
