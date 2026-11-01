//! Main-thread AppKit tests (harness = false: `main` runs on the main thread).

mod ui;

use std::time::Instant;

use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2_app_kit::{
    NSApplication, NSBackgroundColorAttributeName, NSFontPanel, NSButton, NSDocument, NSSearchField, NSSearchFieldCell, NSEventModifierFlags, NSFont, NSFontManager, NSFontTraitMask,
    NSGlyphProperty, NSPasteboard, NSPasteboardTypeString, NSTextInputClient, NSTextStorage,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSUserDefaults, NSError, NSNotFound, NSPoint, NSRange, NSRect, NSSize, NSString,
    ns_string,
};
use prasasti::document::{HoldReason, PrasastiDocument};
use prasasti::editor::{EditorView, make_editor};
use prasasti::find_bar::FindBar;
use prasasti::footer::Footer;
use prasasti::settings;
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

const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;
const OPT_CMD: NSEventModifierFlags = NSEventModifierFlags::Option.union(CMD);

/// Runs `f` with the general pasteboard holding `s`, restoring its string after.
fn with_clipboard<T>(s: &str, f: impl FnOnce() -> T) -> T {
    let pb = NSPasteboard::generalPasteboard();
    let type_ = unsafe { NSPasteboardTypeString };
    let saved = pb.stringForType(type_);
    pb.clearContents();
    pb.setString_forType(&NSString::from_str(s), type_);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    pb.clearContents();
    if let Some(s) = saved {
        pb.setString_forType(&s, type_);
    }
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

fn bold_undoes_in_one_step() {
    let (win, e) = ui::window_with("a word b");
    e.setSelectedRange(NSRange::new(2, 4));
    ui::shortcut(&win, "b", CMD);
    assert_eq!(e.string().to_string(), "a **word** b");
    ui::pump(0.05);
    ui::shortcut(&win, "z", CMD);
    assert_eq!(e.string().to_string(), "a word b");
    win.close();
}

fn bold_typing_round_trip() {
    let (win, e) = ui::window_with("a ");
    e.setSelectedRange(NSRange::new(2, 0));
    ui::shortcut(&win, "b", CMD);
    ui::type_text(&win, "hi");
    ui::shortcut(&win, "b", CMD);
    ui::type_text(&win, " more");
    assert_eq!(e.string().to_string(), "a **hi** more");
    win.close();
}

fn tab_nests_list_item() {
    let (win, e) = ui::window_with("- one\n- two");
    e.setSelectedRange(NSRange::new(11, 0));
    ui::key(&win, "\t", NSEventModifierFlags::empty());
    assert_eq!(e.string().to_string(), "- one\n    - two");
    assert_eq!(e.selectedRange(), NSRange::new(15, 0));
    ui::key(&win, "\t", NSEventModifierFlags::Shift);
    assert_eq!(e.string().to_string(), "- one\n- two");
    assert_eq!(e.selectedRange(), NSRange::new(11, 0));
    win.close();
}

fn tab_outside_list_inserts_tab() {
    let (win, e) = ui::window_with("plain");
    e.setSelectedRange(NSRange::new(5, 0));
    ui::key(&win, "\t", NSEventModifierFlags::empty());
    assert_eq!(e.string().to_string(), "plain\t");
    win.close();
}

fn link_from_clipboard() {
    let text = with_clipboard("https://x.org", || {
        let (win, e) = ui::window_with("site");
        e.setSelectedRange(NSRange::new(0, 4));
        ui::shortcut(&win, "k", CMD);
        let t = e.string().to_string();
        win.close();
        t
    });
    assert_eq!(text, "[site](https://x.org)");
}

fn paste_normalizes_crlf() {
    let text = with_clipboard("a\r\nb", || {
        let (win, e) = ui::window_with("");
        ui::shortcut(&win, "v", CMD);
        let t = e.string().to_string();
        win.close();
        t
    });
    assert_eq!(text, "a\nb");
}

fn find_bar(e: &EditorView) -> Retained<FindBar> {
    e.find_bar().expect("editor has a find bar")
}

/// The bar's button titled `title`, as the user sees it.
fn button(bar: &FindBar, title: &str) -> Retained<NSButton> {
    bar.subviews()
        .iter()
        .find_map(|v| v.downcast::<NSButton>().ok().filter(|b| b.title().to_string() == title))
        .unwrap_or_else(|| panic!("no {title:?} button"))
}

fn click(b: &NSButton) {
    // SAFETY: a nil sender is allowed.
    unsafe { b.performClick(None) };
}

fn match_bg(e: &EditorView, i: usize) -> Option<Retained<AnyObject>> {
    let lm = unsafe { e.layoutManager() }.expect("layout manager");
    // SAFETY: a null effective-range pointer is allowed; `i` is in the text.
    unsafe { lm.temporaryAttribute_atCharacterIndex_effectiveRange(NSBackgroundColorAttributeName, i, std::ptr::null_mut()) }
}

fn is_first_responder(win: &objc2_app_kit::NSWindow, e: &EditorView) -> bool {
    win.firstResponder().is_some_and(|r| std::ptr::eq(&*r, e.as_ref() as &objc2_app_kit::NSResponder))
}

/// Opens the bar on `text` (caret at 0) and types `query` into its search field.
fn searching(text: &str, query: &str, replace: bool) -> (Retained<objc2_app_kit::NSWindow>, Retained<EditorView>) {
    let (win, e) = ui::window_with(text);
    e.setSelectedRange(NSRange::new(0, 0));
    ui::shortcut(&win, "f", if replace { OPT_CMD } else { CMD });
    ui::type_text(&win, query);
    (win, e)
}

fn paints_matches() {
    let (win, e) = searching("tara țară", "tara", false);
    let bar = find_bar(&e);
    assert_eq!(bar.matches(), vec![0, 5]);
    let pal = palette(false);
    assert!(match_bg(&e, 0).is_some_and(|c| pal.current.isEqual(Some(&*c))));
    assert!(match_bg(&e, 5).is_some_and(|c| pal.search.isEqual(Some(&*c))));
    assert!(match_bg(&e, 4).is_none());
    assert_eq!(e.string().to_string(), "tara țară");
    assert_eq!(e.selectedRange(), NSRange::new(0, 4));
    win.close();
}

fn return_steps_matches() {
    let (win, e) = searching("a a a", "a", false);
    assert_eq!(e.selectedRange(), NSRange::new(0, 1));
    ui::key(&win, "\r", NSEventModifierFlags::empty());
    assert_eq!(e.selectedRange(), NSRange::new(2, 1));
    ui::key(&win, "\r", NSEventModifierFlags::Shift);
    assert_eq!(e.selectedRange(), NSRange::new(0, 1));
    ui::shortcut(&win, "g", CMD);
    assert_eq!(e.selectedRange(), NSRange::new(2, 1));
    ui::shortcut(&win, "G", CMD);
    assert_eq!(e.selectedRange(), NSRange::new(0, 1));
    assert_eq!(e.string().to_string(), "a a a");
    win.close();
}

fn replace_all_one_undo() {
    let (win, e) = searching("a a a", "a", true);
    ui::key(&win, "\t", NSEventModifierFlags::empty());
    ui::type_text(&win, "b");
    ui::pump(0.05);
    click(&button(&find_bar(&e), "Replace All"));
    assert_eq!(e.string().to_string(), "b b b");
    assert_eq!(e.selectedRange(), NSRange::new(5, 0));
    assert!(find_bar(&e).matches().is_empty());
    ui::pump(0.05);
    ui::shortcut(&win, "z", CMD);
    assert_eq!(e.string().to_string(), "a a a");
    assert_eq!(e.selectedRange(), NSRange::new(0, 5));
    assert_eq!(find_bar(&e).matches(), vec![0, 2, 4]);
    win.close();
}

fn matches_follow_edits_without_moving_caret() {
    let (win, e) = searching("a", "a", false);
    win.makeFirstResponder(Some(&e));
    e.setSelectedRange(NSRange::new(0, 0));
    ui::type_text(&win, "x ");
    assert_eq!(e.string().to_string(), "x a");
    assert_eq!(e.selectedRange(), NSRange::new(2, 0));
    assert_eq!(find_bar(&e).matches(), vec![2]);
    ui::pump(0.05);
    assert!(match_bg(&e, 2).is_some());
    assert!(match_bg(&e, 0).is_none());
    win.close();
}

fn replace_one_advances() {
    let (win, e) = searching("a a", "a", false);
    let replace = button(&find_bar(&e), "Replace");
    assert!(replace.isHidden(), "find-only mode");
    // ⌥⌘F from the search field switches to find and replace.
    ui::shortcut(&win, "f", OPT_CMD);
    assert!(!replace.isHidden());
    ui::key(&win, "\t", NSEventModifierFlags::empty());
    ui::type_text(&win, "b");
    click(&replace);
    assert_eq!(e.string().to_string(), "b a");
    assert_eq!(e.selectedRange(), NSRange::new(2, 1));
    ui::pump(0.05); // separate clicks, separate undo groups
    click(&replace);
    assert_eq!(e.string().to_string(), "b b");
    assert_eq!(e.selectedRange(), NSRange::new(3, 0));
    ui::pump(0.05);
    ui::shortcut(&win, "z", CMD);
    assert_eq!(e.string().to_string(), "b a");
    assert_eq!(e.selectedRange(), NSRange::new(2, 1));
    win.close();
}

fn escape_closes_bar() {
    let (win, e) = searching("a a", "a", false);
    ui::key(&win, "\u{1b}", NSEventModifierFlags::empty());
    let bar = find_bar(&e);
    assert!(bar.isHidden());
    let content = win.contentView().expect("content");
    content.layoutSubtreeIfNeeded();
    let scroll = e.enclosingScrollView().expect("scroll").frame();
    assert_eq!(scroll.origin.y + scroll.size.height, content.bounds().size.height, "editor takes the bar's room");
    assert!(match_bg(&e, 0).is_none() && match_bg(&e, 2).is_none());
    assert!(is_first_responder(&win, &e));
    assert_eq!(e.string().to_string(), "a a");
    assert_eq!(e.selectedRange(), NSRange::new(0, 1));
    // Reopening searches again for what the field holds.
    ui::shortcut(&win, "f", CMD);
    assert_eq!(bar.matches(), vec![0, 2]);
    win.close();
}

fn replace_all_keeps_far_markers_hidden() {
    let (win, e) = searching("**a** b a", "a", true);
    ui::key(&win, "\t", NSEventModifierFlags::empty());
    ui::type_text(&win, "c");
    click(&button(&find_bar(&e), "Replace All"));
    assert_eq!(e.string().to_string(), "**c** b c");
    assert_eq!(e.selectedRange(), NSRange::new(9, 0));
    assert!([0, 1, 3, 4].iter().all(|&i| hidden(e.storage(), i)));
    win.close();
}

fn clearing_query_leaves_text_alone() {
    let (win, e) = searching("a a a", "a", true);
    let bar = find_bar(&e);
    let field = bar
        .subviews()
        .iter()
        .find_map(|v| v.downcast::<NSSearchField>().ok())
        .expect("search field");
    let cell = field.cell().and_then(|c| c.downcast::<NSSearchFieldCell>().ok()).expect("search cell");
    // The field's cancel (X) button, as a click on it.
    unsafe { cell.cancelButtonCell().expect("cancel button").performClick(Some(&field)) };
    assert_eq!(field.stringValue().to_string(), "");
    assert!(bar.matches().is_empty());
    click(&button(&bar, "Replace All"));
    click(&button(&bar, "Replace"));
    assert_eq!(e.string().to_string(), "a a a");
    assert_eq!(e.selectedRange(), NSRange::new(0, 1));
    // A change that sends no text-did-change (set programmatically) is caught too.
    ui::type_text(&win, "a");
    field.setStringValue(ns_string!(""));
    click(&button(&bar, "Replace All"));
    click(&button(&bar, "Replace"));
    assert_eq!(e.string().to_string(), "a a a");
    assert_eq!(e.selectedRange(), NSRange::new(0, 1));
    win.close();
}

fn undo_in_field_without_replace_stays_in_field() {
    let (win, e) = searching("ab", "ab", false);
    ui::pump(0.05);
    ui::shortcut(&win, "z", CMD);
    let field = find_bar(&e).subviews().iter().find_map(|v| v.downcast::<NSSearchField>().ok()).expect("search field");
    assert_eq!(field.stringValue().to_string(), "");
    assert_eq!(e.string().to_string(), "ab");
    assert_eq!(e.selectedRange(), NSRange::new(0, 2));
    win.close();
}

fn click_moves_current_highlight() {
    let (win, e) = searching("a a", "a", false);
    e.setSelectedRange(NSRange::new(2, 1));
    let pal = palette(false);
    assert!(match_bg(&e, 2).is_some_and(|c| pal.current.isEqual(Some(&*c))));
    assert!(match_bg(&e, 0).is_some_and(|c| pal.search.isEqual(Some(&*c))));
    assert_eq!(e.string().to_string(), "a a");
    win.close();
}

/// Points the settings at the test suite, emptied.
fn fresh_defaults() {
    let d = test_defaults();
    for k in ["editorFont", "textZoom"] {
        d.removeObjectForKey(&NSString::from_str(k));
    }
}

fn test_defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(ns_string!("prasasti-tests"))).expect("suite")
}

fn footer(e: &EditorView) -> Retained<Footer> {
    e.footer().expect("footer registered")
}

fn point_size(e: &EditorView) -> f64 {
    e.font().expect("font").pointSize()
}

fn missing_font_falls_back() {
    let f = settings::editor_font("No Such Font 123", 1.0);
    let family = f.familyName().map(|s| s.to_string()).unwrap_or_default();
    assert!(family == "IBM Plex Mono" || f.isFixedPitch(), "{family}");
    assert_eq!(f.pointSize(), 13.0);
}

fn zoom_in_changes_point_size() {
    fresh_defaults();
    let (win, e) = ui::window_with("# hi **b**");
    assert_eq!(point_size(&e), 13.0);
    ui::shortcut(&win, "=", CMD);
    assert!((point_size(&e) - 13.0 * 1.1).abs() < 1e-9);
    assert_eq!(settings::zoom(), 1.1);
    // The storage is restyled too, not only the typing font.
    let f = attr(e.storage(), unsafe { objc2_app_kit::NSFontAttributeName }, 0).expect("font attr");
    assert!((f.downcast::<NSFont>().unwrap().pointSize() - 13.0 * 1.1).abs() < 1e-9);
    ui::shortcut(&win, "+", CMD);
    assert_eq!(settings::zoom(), 1.25);
    ui::shortcut(&win, "-", CMD);
    assert_eq!(settings::zoom(), 1.1);
    ui::shortcut(&win, "0", CMD);
    assert_eq!(point_size(&e), 13.0);
    assert_eq!(footer(&e).status(), "Text size 100%");
    // Zoom is a view setting: no undo step, text untouched.
    assert!(!e.undoManager().is_some_and(|u| u.canUndo()));
    assert_eq!(e.string().to_string(), "# hi **b**");
    win.close();
}

fn zoom_status_at_limit() {
    fresh_defaults();
    settings::set_zoom(3.0);
    let (win, e) = ui::window_with("x");
    assert_eq!(point_size(&e), 39.0);
    ui::shortcut(&win, "=", CMD);
    assert_eq!(settings::zoom(), 3.0);
    assert_eq!(footer(&e).status(), "Text size 300%");
    win.close();
}

fn status_clears_after_3s() {
    let (win, e) = ui::window_with("x");
    footer(&e).set_status("hello");
    ui::pump(2.5);
    assert_eq!(footer(&e).status(), "hello");
    footer(&e).set_status("again"); // a new status restarts the clock
    ui::pump(2.0);
    assert_eq!(footer(&e).status(), "again");
    ui::pump(1.2);
    assert_eq!(footer(&e).status(), "");
    win.close();
}

fn zoom_applies_to_every_window() {
    fresh_defaults();
    let (w1, e1) = ui::window_with("a");
    let (w2, e2) = ui::window_with("b");
    {
        // A window that is gone must not be notified.
        let (w3, _e3) = ui::window_with("c");
        w3.close();
    }
    ui::shortcut(&w1, "-", CMD);
    assert!((point_size(&e2) - 13.0 * 0.9).abs() < 1e-9);
    assert_eq!(footer(&e1).status(), "Text size 90%");
    assert_eq!(footer(&e2).status(), "");
    w1.close();
    w2.close();
}

fn counts_words_after_debounce() {
    let (win, e) = ui::window_with("");
    ui::type_text(&win, "one two");
    ui::pump(0.1);
    assert_eq!(footer(&e).words(), 0, "debounced");
    ui::pump(0.4);
    assert_eq!(footer(&e).words(), 2);
    assert_eq!(footer(&e).words_label(), "2 Words");
    ui::key(&win, ui::BACKSPACE, NSEventModifierFlags::empty());
    ui::key(&win, ui::BACKSPACE, NSEventModifierFlags::empty());
    ui::key(&win, ui::BACKSPACE, NSEventModifierFlags::empty());
    ui::pump(0.4);
    assert_eq!(footer(&e).words_label(), "1 Word");
    win.close();
}

fn change_font_stores_family() {
    fresh_defaults();
    let (win, e) = ui::window_with("x");
    settings::set_zoom(1.5);
    let mtm = mtm();
    let fm = NSFontManager::sharedFontManager(mtm);
    let menlo = NSFont::fontWithName_size(ns_string!("Menlo"), 30.0).expect("Menlo ships with macOS");
    // The panel is out of process for events; set its choice and send its action.
    let panel = NSFontPanel::sharedFontPanel(mtm);
    panel.setPanelFont_isMultiple(&menlo, false);
    // What a pick in the panel does: the manager sends changeFont: (aimed at
    // the editor; the test app has no key window) and converts via the panel.
    unsafe {
        fm.setTarget(Some(&e));
        fm.modifyFontViaPanel(Some(&panel));
        fm.setTarget(None);
    }
    assert_eq!(settings::font_family(), "Menlo");
    assert_eq!(e.font().unwrap().familyName().unwrap().to_string(), "Menlo");
    assert_eq!(point_size(&e), 13.0 * 1.5, "size comes from the zoom");
    panel.close();
    win.close();
}

fn font_button_opens_panel() {
    let (win, e) = ui::window_with("x");
    let b = footer(&e).subviews().iter().find_map(|v| v.downcast::<NSButton>().ok()).expect("font button");
    click(&b);
    let panel = NSFontPanel::sharedFontPanel(mtm());
    assert!(panel.isVisible());
    panel.close();
    ui::shortcut(&win, "t", CMD);
    assert!(panel.isVisible(), "⌘T shows it too");
    panel.close();
    win.close();
}

fn main() {
    let _app = NSApplication::sharedApplication(mtm());
    // Never the user's real defaults domain.
    settings::set_defaults(Some(test_defaults()));
    fresh_defaults();
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
        ("bold_undoes_in_one_step", bold_undoes_in_one_step),
        ("bold_typing_round_trip", bold_typing_round_trip),
        ("tab_nests_list_item", tab_nests_list_item),
        ("tab_outside_list_inserts_tab", tab_outside_list_inserts_tab),
        ("link_from_clipboard", link_from_clipboard),
        ("paste_normalizes_crlf", paste_normalizes_crlf),
        ("paints_matches", paints_matches),
        ("return_steps_matches", return_steps_matches),
        ("replace_all_one_undo", replace_all_one_undo),
        ("matches_follow_edits_without_moving_caret", matches_follow_edits_without_moving_caret),
        ("replace_one_advances", replace_one_advances),
        ("escape_closes_bar", escape_closes_bar),
        ("clearing_query_leaves_text_alone", clearing_query_leaves_text_alone),
        ("click_moves_current_highlight", click_moves_current_highlight),
        ("undo_in_field_without_replace_stays_in_field", undo_in_field_without_replace_stays_in_field),
        ("replace_all_keeps_far_markers_hidden", replace_all_keeps_far_markers_hidden),
        ("missing_font_falls_back", missing_font_falls_back),
        ("zoom_in_changes_point_size", zoom_in_changes_point_size),
        ("zoom_status_at_limit", zoom_status_at_limit),
        ("status_clears_after_3s", status_clears_after_3s),
        ("zoom_applies_to_every_window", zoom_applies_to_every_window),
        ("counts_words_after_debounce", counts_words_after_debounce),
        ("change_font_stores_family", change_font_stores_family),
        ("font_button_opens_panel", font_button_opens_panel),
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
