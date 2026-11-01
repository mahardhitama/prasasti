//! Event-driven helpers shared by the app tests.
#![allow(dead_code)]

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSEvent, NSEventModifierFlags, NSEventType, NSMenu,
    NSMenuItem, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSDate, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSSize, NSString};
use prasasti::editor::{EditorView, editor_in_scroll};
use prasasti::menu;

pub const LEFT: &str = "\u{F702}";
pub const RIGHT: &str = "\u{F703}";
pub const UP: &str = "\u{F700}";
pub const DOWN: &str = "\u{F701}";
pub const BACKSPACE: &str = "\u{7f}";

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().unwrap()
}

/// An off-screen window (never ordered front) holding the editor in a scroll
/// view, the editor first responder, and the app's main menu installed.
pub fn window_with(text: &str) -> (Retained<NSWindow>, Retained<EditorView>) {
    let mtm = mtm();
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
    // SAFETY: released-when-closed is turned off right below.
    let win = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { win.setReleasedWhenClosed(false) };
    let (scroll, editor) = editor_in_scroll(frame);
    editor.setString(&NSString::from_str(text));
    win.setContentView(Some(&scroll));
    win.makeFirstResponder(Some(&editor));
    menu::install(&NSApplication::sharedApplication(mtm), mtm);
    (win, editor)
}

fn key_code(key: &str) -> u16 {
    match key {
        "\t" => 48,
        "\r" => 36,
        "\u{1b}" => 53,
        BACKSPACE => 51,
        LEFT => 123,
        RIGHT => 124,
        DOWN => 125,
        UP => 126,
        _ => 0,
    }
}

fn event(win: &NSWindow, kind: NSEventType, key: &str, mods: NSEventModifierFlags) -> Retained<NSEvent> {
    let mut mods = mods;
    if [LEFT, RIGHT, UP, DOWN].contains(&key) {
        mods |= NSEventModifierFlags::NumericPad | NSEventModifierFlags::Function;
    }
    let s = NSString::from_str(key);
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        kind, NSPoint::new(0.0, 0.0), mods, 0.0, win.windowNumber(), None, &s, &s, false, key_code(key),
    )
    .expect("key event")
}

/// Sends one key-down/key-up pair through the window.
pub fn key(win: &NSWindow, key: &str, mods: NSEventModifierFlags) {
    win.sendEvent(&event(win, NSEventType::KeyDown, key, mods));
    win.sendEvent(&event(win, NSEventType::KeyUp, key, mods));
}

/// Types `s`, one key-down/key-up pair per character.
pub fn type_text(win: &NSWindow, s: &str) {
    for c in s.chars() {
        key(win, c.encode_utf8(&mut [0; 4]), NSEventModifierFlags::empty());
    }
}

/// Key equivalent with Shift spelled as a flag: "Z" + ⌘ == "z" + ⇧⌘.
fn normalise(key: &str, mods: NSEventModifierFlags) -> (String, NSEventModifierFlags) {
    let mods = mods & NSEventModifierFlags::DeviceIndependentFlagsMask;
    if key.chars().any(char::is_uppercase) {
        (key.to_lowercase(), mods | NSEventModifierFlags::Shift)
    } else {
        (key.to_string(), mods)
    }
}

fn find_item(menu: &NSMenu, want: &(String, NSEventModifierFlags)) -> Option<Retained<NSMenuItem>> {
    menu.itemArray().iter().find_map(|item| match item.submenu() {
        Some(sub) => find_item(&sub, want),
        None => (normalise(&item.keyEquivalent().to_string(), item.keyEquivalentModifierMask()) == *want
            && item.action().is_some())
        .then_some(item),
    })
}

/// Runs the main-menu item bound to `key` + `mods`, sending its action to the
/// first responder in the window's chain that implements it, as a nil-targeted
/// menu action would. Panics if no item has that binding or nothing handled it.
/// (The test app is never active, so the window is never key and
/// `performKeyEquivalent` would send nil-targeted actions nowhere.)
pub fn shortcut(win: &NSWindow, key: &str, mods: NSEventModifierFlags) {
    let app = NSApplication::sharedApplication(mtm());
    let menu = app.mainMenu().expect("main menu");
    let item = find_item(&menu, &normalise(key, mods))
        .unwrap_or_else(|| panic!("no menu item bound to {key:?} {mods:?}"));
    let action = item.action().expect("item action");
    // AppKit's explicit-target send does not walk the chain, so find the target here.
    let mut target = win.firstResponder();
    while let Some(r) = target.as_deref().filter(|r| !r.respondsToSelector(action)) {
        // SAFETY: a plain getter of the responder chain.
        target = unsafe { r.nextResponder() };
    }
    let target = target.unwrap_or_else(|| panic!("no responder for {action:?} ({key:?} {mods:?})"));
    // SAFETY: menu actions are `-(void)x:(id)sender`; the sender is the item.
    let sent = unsafe { app.sendAction_to_from(action, Some(target.as_ref()), Some(item.as_ref())) };
    assert!(sent, "nothing handled {action:?} for {key:?} {mods:?}");
}

/// Runs the main run loop for `secs` (timers, deferred work).
pub fn pump(secs: f64) {
    NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(secs));
}
