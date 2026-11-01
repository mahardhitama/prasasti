//! Event-driven helpers shared by the app tests.
#![allow(dead_code)]

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSMenu,
    NSMenuItem, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSDate, NSDefaultRunLoopMode, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSSize, NSString};
use prasasti::document::window_content;
use prasasti::editor::EditorView;
use prasasti::menu;

pub const LEFT: &str = "\u{F702}";
pub const RIGHT: &str = "\u{F703}";
pub const UP: &str = "\u{F700}";
pub const DOWN: &str = "\u{F701}";
pub const BACKSPACE: &str = "\u{7f}";

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().unwrap()
}

/// An off-screen window (never ordered front) holding the document window's
/// content (find bar, editor in a scroll view), the editor first responder, and the app's main menu installed.
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
    let (content, editor) = window_content(frame);
    editor.setString(&NSString::from_str(text));
    win.setContentView(Some(&content));
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
    // The keyboard reports Shift-Tab as the backtab character (U+0019).
    let chars = if key == "\t" && mods.contains(NSEventModifierFlags::Shift) { "\u{19}" } else { key };
    let s = NSString::from_str(chars);
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        kind, NSPoint::new(0.0, 0.0), mods, 0.0, win.windowNumber(), None, &s, &s, false, key_code(key),
    )
    .expect("key event")
}

/// Sends `ev` to the window as NSApp's current event, as a real key press is
/// (field-editor delegates read modifiers such as Shift-Return from it).
fn send(win: &NSWindow, ev: Retained<NSEvent>) {
    let app = NSApplication::sharedApplication(mtm());
    app.postEvent_atStart(&ev, true);
    let ev = app
        .nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, None, unsafe { NSDefaultRunLoopMode }, true)
        .expect("posted event");
    win.sendEvent(&ev);
}

/// Sends one key-down/key-up pair through the window.
pub fn key(win: &NSWindow, key: &str, mods: NSEventModifierFlags) {
    send(win, event(win, NSEventType::KeyDown, key, mods));
    send(win, event(win, NSEventType::KeyUp, key, mods));
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
/// item's own target if it has one, else to the first responder in the
/// window's chain that implements it, as a nil-targeted menu action would. Panics if no item has that binding or nothing handled it.
/// (The test app is never active, so the window is never key and
/// `performKeyEquivalent` would send nil-targeted actions nowhere.)
pub fn shortcut(win: &NSWindow, key: &str, mods: NSEventModifierFlags) {
    let app = NSApplication::sharedApplication(mtm());
    let menu = app.mainMenu().expect("main menu");
    let item = find_item(&menu, &normalise(key, mods))
        .unwrap_or_else(|| panic!("no menu item bound to {key:?} {mods:?}"));
    let action = item.action().expect("item action");
    if let Some(target) = item.target() {
        // SAFETY: as below; an item with its own target ignores the chain.
        let sent = unsafe { app.sendAction_to_from(action, Some(&target), Some(item.as_ref())) };
        assert!(sent, "{target:?} did not handle {action:?}");
        return;
    }
    // AppKit's explicit-target send does not walk the chain, so find the target here.
    let mut target = win.firstResponder();
    while let Some(r) = target.as_deref().filter(|r| !r.respondsToSelector(action)) {
        // SAFETY: a plain getter of the responder chain.
        target = unsafe { r.nextResponder() };
    }
    // After the chain, AppKit tries the window controller's document.
    let target: Retained<objc2::runtime::AnyObject> = match target {
        Some(r) => r.into(),
        None => win
            .windowController()
            .and_then(|c| unsafe { c.document() }) // SAFETY: a plain getter.
            .filter(|d| d.class().responds_to(action))
            .unwrap_or_else(|| panic!("no responder for {action:?} ({key:?} {mods:?})")),
    };
    // SAFETY: menu actions are `-(void)x:(id)sender`; the sender is the item.
    let sent = unsafe { app.sendAction_to_from(action, Some(&target), Some(item.as_ref())) };
    assert!(sent, "nothing handled {action:?} for {key:?} {mods:?}");
}

/// Runs the main run loop for `secs` (timers, deferred work).
pub fn pump(secs: f64) {
    NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(secs));
}
