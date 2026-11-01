//! Main-thread AppKit tests (harness = false: `main` runs on the main thread).

mod ui;

use std::time::Instant;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplication, NSDocument, NSEventModifierFlags, NSFont, NSFontManager, NSFontTraitMask,
    NSGlyphProperty, NSPasteboard, NSPasteboardTypeString, NSTextInputClient, NSTextStorage,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSError, NSNotFound, NSPoint, NSRange, NSRect, NSSize, NSString,
    ns_string,
};
use prasasti::document::{HoldReason, PrasastiDocument};
use prasasti::editor::{EditorView, make_editor};
use prasasti::styler::{Styler, hidden_key};
use prasasti::theme::{base_font, palette};

const MD: &str = "net.daringfireball.markdown";

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().unwrap()
}

fn read(bytes: &[u8]) -> (Retained<PrasastiDocument>, Result<(), Retained<NSError>>) {
    let d = PrasastiDocument::new(mtm());
    let r = d.readFromData_ofType_error(&NSData::with_bytes(bytes), ns_string!(MD));
    (d, r)
}

fn doc_from(bytes: &[u8]) -> Retained<PrasastiDocument> {
    let (d, r) = read(bytes);
    r.expect("read should succeed");
    d
}

fn read_err(bytes: &[u8]) -> Retained<NSError> {
    read(bytes).1.expect_err("read should fail")
}

fn data_of(d: &PrasastiDocument) -> Vec<u8> {
    let doc: &NSDocument = d;
    doc.dataOfType_error(ns_string!(MD)).expect("data").to_vec()
}

fn opens_utf8() {
    let d = doc_from(b"# hi\n");
    assert_eq!(d.text().to_string(), "# hi\n");
    assert!(d.hold().is_none());
}

fn refuses_non_utf8() {
    let e = read_err(&[0xff, 0x00]);
    assert_eq!(e.localizedDescription().to_string(), "The file isn't UTF-8 text.");
}

fn crlf_holds_autosave() {
    let d = doc_from(b"a\r\nb");
    assert_eq!(d.text().to_string(), "a\nb");
    assert_eq!(d.hold(), Some(HoldReason::LineEndings));
}

fn writes_utf8_lf() {
    let d = doc_from("é\n".as_bytes());
    assert_eq!(data_of(&d), "é\n".as_bytes());
}

fn window_shows_text_and_typing_edits() {
    let d = doc_from(b"x");
    d.makeWindowControllers();
    let editor = d.editor().expect("editor after makeWindowControllers");
    assert_eq!(editor.string().to_string(), "x");
    assert!(!d.isDocumentEdited());
    editor.setSelectedRange(NSRange::new(1, 0));
    // SAFETY: an NSString is a valid argument; NSNotFound means "the selection".
    unsafe { editor.insertText_replacementRange(ns_string!("y\r\n"), NSRange::new(NSNotFound as usize, 0)) };
    assert_eq!(d.text().to_string(), "xy\r\n");
    // NSWindow routes undo to its controller's document; typing must use that
    // manager, which drives "edited" and autosave. (Edited itself flips only
    // when the run loop closes the undo group, so it is not asserted here.)
    assert_eq!(editor.undoManager(), d.undoManager());
    assert_eq!(data_of(&d), b"xy\n");
    d.close();
}

fn editor_with(text: &str) -> Retained<EditorView> {
    let e = make_editor(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0)));
    e.setString(&NSString::from_str(text));
    e
}

fn styled(text: &str, caret: usize) -> Retained<EditorView> {
    let e = editor_with(text);
    Styler::default().restyle_all(e.storage(), &base_font(), &palette(false), caret);
    e
}

fn attr(s: &NSTextStorage, key: &NSString, i: usize) -> Option<Retained<AnyObject>> {
    // SAFETY: a null effective-range pointer is allowed.
    unsafe { s.attribute_atIndex_effectiveRange(key, i, std::ptr::null_mut()) }
}

fn hidden(s: &NSTextStorage, i: usize) -> bool {
    attr(s, hidden_key(), i).is_some()
}

fn is_bold(s: &NSTextStorage, i: usize) -> bool {
    let font = attr(s, unsafe { objc2_app_kit::NSFontAttributeName }, i)
        .and_then(|f| f.downcast::<NSFont>().ok())
        .expect("font attribute");
    NSFontManager::sharedFontManager(mtm()).traitsOfFont(&font).contains(NSFontTraitMask::BoldFontMask)
}

fn has_code_bg(s: &NSTextStorage, i: usize) -> bool {
    attr(s, unsafe { objc2_app_kit::NSBackgroundColorAttributeName }, i).is_some()
}

fn fence_settles() {
    let e = editor_with("a\n```\nb **c**\n");
    let s = e.storage();
    assert!(has_code_bg(s, 6) && !is_bold(s, 10), "inside the fence: code");
    // A closing fence right after the opening one turns "b **c**" back into prose.
    s.replaceCharactersInRange_withString(NSRange::new(6, 0), ns_string!("```\n"));
    assert_eq!(e.string().to_string(), "a\n```\n```\nb **c**\n");
    assert!(!has_code_bg(s, 10), "b no longer code");
    assert!(is_bold(s, 14), "c bold");
    assert!(hidden(s, 12) && hidden(s, 13) && hidden(s, 15) && hidden(s, 16), "markers hidden");
}

fn hidden_markers_have_null_glyphs() {
    let e = styled("x **b** y", 0);
    let lm = unsafe { e.layoutManager() }.expect("layout manager");
    let prop = |c| lm.propertyForGlyphAtIndex(lm.glyphIndexForCharacterAtIndex(c));
    for c in [2, 3, 5, 6] {
        assert!(prop(c).contains(NSGlyphProperty::Null), "char {c} null glyph");
    }
    assert!(!prop(4).contains(NSGlyphProperty::Null), "char 4 visible");
}

fn markers_show_at_caret() {
    let (win, e) = ui::window_with("x **b** y");
    let s = e.storage();
    e.setSelectedRange(NSRange::new(0, 0));
    assert!([2, 3, 5, 6].iter().all(|&i| hidden(s, i)), "hidden with caret at 0");
    let right = || ui::key(&win, ui::RIGHT, NSEventModifierFlags::empty());
    right();
    assert_eq!(e.selectedRange().location, 1);
    right();
    assert_eq!(e.selectedRange().location, 2, "stops before the opening markers");
    assert!(!(2..7).any(|i| hidden(s, i)), "shown with caret against the markup");
    right();
    assert_eq!(e.selectedRange().location, 3);
    for _ in 0..5 {
        right();
    }
    assert_eq!(e.selectedRange().location, 8);
    assert!([2, 3, 5, 6].iter().all(|&i| hidden(s, i)), "hidden again");
    win.close();
}

fn typing_before_bold_stays_outside() {
    let (win, e) = ui::window_with("x **b** y");
    e.setSelectedRange(NSRange::new(1, 0));
    ui::key(&win, ui::RIGHT, NSEventModifierFlags::empty());
    ui::type_text(&win, "new ");
    assert_eq!(e.string().to_string(), "x new **b** y");
    win.close();
}

fn end_of_line_lands_after_closing_markers() {
    let (win, e) = ui::window_with("text **b**\nnext");
    e.setSelectedRange(NSRange::new(0, 0));
    ui::key(&win, ui::RIGHT, NSEventModifierFlags::Command);
    assert_eq!(e.selectedRange(), NSRange::new(10, 0));
    ui::type_text(&win, "x");
    assert_eq!(e.string().to_string(), "text **b**x\nnext");
    win.close();
}

fn paste_ending_in_markers_keeps_caret_after() {
    let pb = NSPasteboard::generalPasteboard();
    let type_ = unsafe { NSPasteboardTypeString };
    let saved = pb.stringForType(type_);
    pb.clearContents();
    pb.setString_forType(ns_string!("**b**"), type_);
    let (win, e) = ui::window_with("a ");
    e.setSelectedRange(NSRange::new(2, 0));
    ui::shortcut(&win, "v", NSEventModifierFlags::Command);
    let caret = e.selectedRange();
    ui::type_text(&win, "c");
    let text = e.string().to_string();
    win.close();
    pb.clearContents();
    if let Some(s) = saved {
        pb.setString_forType(&s, type_);
    }
    assert_eq!(caret, NSRange::new(7, 0));
    assert_eq!(text, "a **b**c");
}

fn shortcut_selects_all() {
    let (win, e) = ui::window_with("abc");
    e.setSelectedRange(NSRange::new(1, 0));
    ui::shortcut(&win, "a", NSEventModifierFlags::Command);
    assert_eq!(e.selectedRange(), NSRange::new(0, 3));
    win.close();
}

/// Deleting a paragraph shrinks the state cache; undo grows it back.
fn deleting_fence_line_resyncs_cache() {
    let (win, e) = ui::window_with("a\n```\nb **c**\n```\nd **e**");
    let s = e.storage();
    let original = |s: &NSTextStorage| {
        assert!(has_code_bg(s, 6) && !is_bold(s, 10), "b **c** is code");
        assert!(!has_code_bg(s, 18) && is_bold(s, 22), "d **e** is prose");
    };
    original(s);
    e.setSelectedRange(NSRange::new(2, 4));
    ui::key(&win, ui::BACKSPACE, NSEventModifierFlags::empty());
    assert_eq!(e.string().to_string(), "a\nb **c**\n```\nd **e**");
    assert!(!has_code_bg(s, 2) && is_bold(s, 6), "b **c** prose and bold");
    assert!([4, 5, 7, 8].iter().all(|&i| hidden(s, i)), "its markers hidden");
    assert!(has_code_bg(s, 14) && !is_bold(s, 18), "d **e** now code");
    ui::pump(0.05);
    ui::shortcut(&win, "z", NSEventModifierFlags::Command);
    assert_eq!(e.string().to_string(), "a\n```\nb **c**\n```\nd **e**");
    original(s);
    win.close();
}

/// Typed text is styled through the storage delegate, not just on load.
fn typing_styles_live() {
    let (win, e) = ui::window_with("");
    ui::type_text(&win, "a **b** c");
    assert_eq!(e.string().to_string(), "a **b** c");
    let s = e.storage();
    assert!(is_bold(s, 4));
    assert!([2, 3, 5, 6].iter().all(|&i| hidden(s, i)), "markers hidden after caret left");
    win.close();
}

fn styles_bold_after_emoji() {
    let e = styled("😀 **b**", 0);
    assert!(is_bold(e.storage(), 5));
    assert!(!is_bold(e.storage(), 2));
}

fn large_doc() -> String {
    (0..30_000).map(|n| format!("line {n} with **bold** and a [link](u)\n")).collect()
}

/// Debug builds are several times slower than release.
const PERF_LIMIT: f64 = if cfg!(debug_assertions) { 5.0 } else { 1.0 };

fn restyles_2mb_under_1s() {
    let e = editor_with(&large_doc());
    let t = Instant::now();
    Styler::default().restyle_all(e.storage(), &base_font(), &palette(false), 0);
    let secs = t.elapsed().as_secs_f64();
    println!("    restyle_all of {} UTF-16 units: {secs:.3}s", e.string().length());
    assert!(secs < PERF_LIMIT, "{secs}s");
}

fn keystroke_in_large_doc_restyles_locally() {
    let e = editor_with(&large_doc());
    let s = e.storage();
    let (font, pal) = (base_font(), palette(false));
    let mut st = Styler::default();
    st.restyle_all(s, &font, &pal, 0);
    let mid = e.string().length() / 2;
    s.replaceCharactersInRange_withString(NSRange::new(mid, 0), ns_string!("x"));
    let r = st.restyle_edit(s, &font, &pal, NSRange::new(mid, 1), mid + 1);
    assert!(r.length < 200, "restyled {r:?}");
    assert!(r.location <= mid && mid < r.location + r.length);
}

fn main() {
    let _app = NSApplication::sharedApplication(mtm());
    let tests: &[(&str, fn())] = &[
        ("opens_utf8", opens_utf8),
        ("refuses_non_utf8", refuses_non_utf8),
        ("crlf_holds_autosave", crlf_holds_autosave),
        ("writes_utf8_lf", writes_utf8_lf),
        ("window_shows_text_and_typing_edits", window_shows_text_and_typing_edits),
        ("fence_settles", fence_settles),
        ("hidden_markers_have_null_glyphs", hidden_markers_have_null_glyphs),
        ("markers_show_at_caret", markers_show_at_caret),
        ("typing_before_bold_stays_outside", typing_before_bold_stays_outside),
        ("end_of_line_lands_after_closing_markers", end_of_line_lands_after_closing_markers),
        ("paste_ending_in_markers_keeps_caret_after", paste_ending_in_markers_keeps_caret_after),
        ("shortcut_selects_all", shortcut_selects_all),
        ("deleting_fence_line_resyncs_cache", deleting_fence_line_resyncs_cache),
        ("typing_styles_live", typing_styles_live),
        ("styles_bold_after_emoji", styles_bold_after_emoji),
        ("restyles_2mb_under_1s", restyles_2mb_under_1s),
        ("keystroke_in_large_doc_restyles_locally", keystroke_in_large_doc_restyles_locally),
    ];
    let mut failed = 0;
    for (name, f) in tests {
        let ok = autoreleasepool(|_| std::panic::catch_unwind(f).is_ok());
        println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
        failed += usize::from(!ok);
    }
    println!("\n{} passed; {failed} failed", tests.len() - failed);
    if failed > 0 {
        std::process::exit(1);
    }
}
