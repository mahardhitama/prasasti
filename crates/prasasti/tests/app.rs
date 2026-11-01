//! Main-thread AppKit tests (harness = false: `main` runs on the main thread).

mod ui;

use std::time::Instant;

use objc2::{AnyThread, DefinedClass};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2_app_kit::{
    NSApplication, NSBackgroundColorAttributeName, NSFontPanel, NSButton, NSDocument, NSSearchField, NSSearchFieldCell, NSEventModifierFlags, NSFont, NSFontManager, NSFontTraitMask,
    NSGlyphProperty, NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString, NSTextView, NSTextInputClient, NSTextStorage,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSURL, NSUserDefaults, NSError, NSNotFound, NSPoint, NSRange, NSRect, NSSize, NSString,
    ns_string,
};
use prasasti::content::{DocumentView, Mode};
use prasasti::document::{self, HoldReason, PrasastiDocument};
use prasasti::editor::{EditorView, make_editor};
use prasasti::find_bar::FindBar;
use prasasti::preview::PreviewView;
use prasasti::status::StatusPill;
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

/// Runs `f` with the general pasteboard holding `s`, restoring every item
/// and type it held before (the user's clipboard survives the test run).
fn with_clipboard<T>(s: &str, f: impl FnOnce() -> T) -> T {
    use objc2_app_kit::{NSPasteboardItem, NSPasteboardWriting};
    let pb = NSPasteboard::generalPasteboard();
    let saved: Vec<Vec<_>> = pb.pasteboardItems().map(|items| {
        items.iter().map(|item| item.types().iter().filter_map(|t| Some((item.dataForType(&t)?, t))).collect()).collect()
    }).unwrap_or_default();
    pb.clearContents();
    pb.setString_forType(&NSString::from_str(s), unsafe { NSPasteboardTypeString });
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    pb.clearContents();
    let items: Vec<_> = saved.iter().map(|types| {
        let item = NSPasteboardItem::new();
        for (data, t) in types {
            item.setData_forType(data, t);
        }
        objc2::runtime::ProtocolObject::<dyn NSPasteboardWriting>::from_retained(item)
    }).collect();
    if !items.is_empty() {
        pb.writeObjects(&objc2_foundation::NSArray::from_retained_slice(&items));
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

/// Paste and Match Style (⌥⇧⌘V) pastes plain text through our paste: LF endings.
fn paste_and_match_style_normalizes_crlf() {
    let text = with_clipboard("a\r\nb", || {
        let (win, e) = ui::window_with("");
        ui::shortcut(&win, "V", OPT_CMD);
        let t = e.string().to_string();
        win.close();
        t
    });
    assert_eq!(text, "a\nb");
}

/// The app menu has the standard Services, Hide Others (⌥⌘H) and Show All.
fn app_menu_has_standard_items() {
    let (win, _) = ui::window_with("");
    let app = NSApplication::sharedApplication(mtm());
    let app_menu = app.mainMenu().unwrap().itemAtIndex(0).and_then(|i| i.submenu()).expect("app menu");
    let by_action = |a: objc2::runtime::Sel| app_menu.itemArray().iter().find(|i| i.action() == Some(a));
    let others = by_action(objc2::sel!(hideOtherApplications:)).expect("Hide Others");
    assert_eq!(others.keyEquivalent().to_string(), "h");
    assert_eq!(others.keyEquivalentModifierMask(), OPT_CMD);
    assert!(by_action(objc2::sel!(unhideAllApplications:)).is_some(), "Show All");
    assert!(app.servicesMenu().is_some(), "services menu registered");
    let services = app_menu.itemWithTitle(ns_string!("Services")).and_then(|i| i.submenu());
    assert!(services.is_some(), "Services in the app menu");
    win.close();
}

fn copy_as_html_writes_both_types() {
    let (html, text, status) = with_clipboard("old", || {
        let (win, e) = ui::window_with("**b**");
        e.setSelectedRange(NSRange::new(0, 1)); // the whole document is copied, not the selection
        ui::shortcut(&win, "c", CMD | NSEventModifierFlags::Shift);
        let pb = NSPasteboard::generalPasteboard();
        let (html, text) = unsafe { (pb.stringForType(NSPasteboardTypeHTML), pb.stringForType(NSPasteboardTypeString)) };
        let status = status(&e).text();
        win.close();
        (html.expect("html").to_string(), text.expect("text").to_string(), status)
    });
    assert!(html.contains("<strong>b</strong>"), "{html}");
    assert_eq!(text, "**b**");
    assert_eq!(status, "Copied as HTML.");
}

/// ⌘Z after a paste undoes the paste alone, not the typing before it.
fn paste_undoes_alone() {
    let text = with_clipboard("XY", || {
        let (win, e) = ui::window_with("ab");
        e.setSelectedRange(NSRange::new(2, 0));
        ui::type_text(&win, "c");
        ui::pump(0.05);
        ui::shortcut(&win, "v", CMD);
        ui::pump(0.05);
        ui::type_text(&win, "d");
        ui::pump(0.05);
        assert_eq!(e.string().to_string(), "abcXYd");
        ui::shortcut(&win, "z", CMD);
        assert_eq!(e.string().to_string(), "abcXY", "undo removes the typing after the paste only");
        ui::shortcut(&win, "z", CMD);
        let t = e.string().to_string();
        win.close();
        t
    });
    assert_eq!(text, "abc", "undo removes the paste alone");
}

fn print_view(d: &PrasastiDocument) -> (Retained<objc2_app_kit::NSPrintOperation>, Retained<NSTextView>) {
    let doc: &NSDocument = d;
    // SAFETY: an empty dictionary is a valid settings dictionary.
    let op = unsafe { doc.printOperationWithSettings_error(&objc2_foundation::NSDictionary::new()) }.expect("operation");
    let tv = op.view().expect("view").downcast::<NSTextView>().expect("a text view");
    (op, tv)
}

fn print_operation_builds() {
    let d = doc_from(b"# T\n\nbody **x** caf\xc3\xa9\n");
    let (op, tv) = print_view(&d);
    let s = tv.string().to_string();
    assert!(s.contains("T\n") || s.starts_with('T'), "{s:?}");
    assert!(s.contains("body x caf\u{e9}"), "{s:?}"); // markers rendered away, UTF-8 intact
    let info = op.printInfo();
    let width = info.paperSize().width - info.leftMargin() - info.rightMargin();
    assert_eq!(tv.frame().size.width, width);
    assert!(tv.isVerticallyResizable() && !tv.isHorizontallyResizable());
    assert!(tv.frame().size.height > 20.0, "laid out to its full height");
    // SAFETY: plain getters; a null range pointer is allowed.
    let colour = unsafe { tv.textStorage().unwrap().attribute_atIndex_effectiveRange(objc2_app_kit::NSForegroundColorAttributeName, 0, std::ptr::null_mut()) };
    let colour: Retained<objc2_app_kit::NSColor> = colour.expect("a colour").downcast().expect("an NSColor");
    let rgb = colour.colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace()).expect("rgb");
    assert!(rgb.redComponent() < 0.01 && rgb.greenComponent() < 0.01 && rgb.blueComponent() < 0.01);
    assert_eq!(tv.backgroundColor(), objc2_app_kit::NSColor::whiteColor());
}

/// Print uses the editor's font family (the HTML importer alone falls back to
/// Times), keeps bold, and starts at the top of the page.
fn print_uses_editor_font_top_aligned() {
    let d = doc_from(b"# Head\n\nbody **bold** `code`\n");
    let (op, tv) = print_view(&d);
    let family = base_font().familyName().expect("editor family").to_string();
    // SAFETY: a plain getter.
    let storage = unsafe { tv.textStorage() }.unwrap();
    let mut at = 0;
    while at < storage.length() {
        let mut run = NSRange::new(0, 0);
        // SAFETY: NSFontAttributeName is AppKit's key; `at` is in bounds.
        let font = unsafe { storage.attribute_atIndex_effectiveRange(objc2_app_kit::NSFontAttributeName, at, &mut run) };
        let font: Retained<NSFont> = font.expect("a font").downcast().expect("an NSFont");
        assert_eq!(font.familyName().map(|f| f.to_string()), Some(family.clone()), "at {at}");
        at = run.end().max(at + 1);
    }
    let s = storage.string().to_string();
    let bold_at = s.find("bold").unwrap();
    assert!(is_bold(&storage, s[..bold_at].encode_utf16().count()), "bold kept");
    assert!(!op.printInfo().isVerticallyCentered(), "top-aligned");
    assert!(!op.printInfo().isHorizontallyCentered());
}

/// A remote image that never answers delays print by seconds, not ~30 s.
fn print_remote_image_times_out() {
    // A local server that accepts and never answers.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let held: Vec<_> = server.incoming().collect();
        drop(held);
    });
    let d = doc_from(format!("before\n\n![x](http://127.0.0.1:{port}/x.png)\n\nafter\n").as_bytes());
    let start = Instant::now();
    let (_, tv) = print_view(&d);
    let secs = start.elapsed().as_secs_f64();
    assert!(tv.string().to_string().contains("after"));
    assert!(secs < 10.0, "print took {secs:.1} s");
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
    for k in ["editorFont", "textZoom", "columnWidth", "appearance"] {
        d.removeObjectForKey(&NSString::from_str(k));
    }
}

fn test_defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(ns_string!("prasasti-tests"))).expect("suite")
}

fn status(e: &EditorView) -> Retained<StatusPill> {
    e.status().expect("status pill registered")
}

fn point_size(e: &EditorView) -> f64 {
    e.font().expect("font").pointSize()
}

fn missing_font_falls_back() {
    let f = settings::editor_font("No Such Font 123", 1.0);
    let family = f.familyName().map(|s| s.to_string()).unwrap_or_default();
    assert!(family == "JetBrains Mono" || f.isFixedPitch(), "{family}");
    assert_eq!(f.pointSize(), 15.0);
}

fn default_font_is_jetbrains_mono_15() {
    fresh_defaults();
    let f = settings::current_font();
    assert_eq!(f.familyName().map(|s| s.to_string()).as_deref(), Some("JetBrains Mono"));
    assert_eq!(f.pointSize(), 15.0);
}

fn zoom_in_changes_point_size() {
    fresh_defaults();
    let (win, e) = ui::window_with("# hi **b**");
    assert_eq!(point_size(&e), 15.0);
    ui::shortcut(&win, "=", CMD);
    assert!((point_size(&e) - 15.0 * 1.1).abs() < 1e-9);
    assert_eq!(settings::zoom(), 1.1);
    // The storage is restyled too, not only the typing font.
    let f = attr(e.storage(), unsafe { objc2_app_kit::NSFontAttributeName }, 0).expect("font attr");
    assert!((f.downcast::<NSFont>().unwrap().pointSize() - 15.0 * 1.1).abs() < 1e-9);
    ui::shortcut(&win, "+", CMD);
    assert_eq!(settings::zoom(), 1.25);
    ui::shortcut(&win, "-", CMD);
    assert_eq!(settings::zoom(), 1.1);
    ui::shortcut(&win, "0", CMD);
    assert_eq!(point_size(&e), 15.0);
    assert_eq!(status(&e).text(), "Text size 100%");
    // Zoom is a view setting: no undo step, text untouched.
    assert!(!e.undoManager().is_some_and(|u| u.canUndo()));
    assert_eq!(e.string().to_string(), "# hi **b**");
    win.close();
}

fn zoom_flashes_text_size() {
    fresh_defaults();
    settings::set_zoom(3.0);
    let (win, e) = ui::window_with("x");
    assert_eq!(point_size(&e), 45.0);
    ui::shortcut(&win, "=", CMD);
    assert_eq!(settings::zoom(), 3.0);
    assert_eq!(status(&e).text(), "Text size 300%");
    win.close();
}

fn status_flash_clears_after_2s() {
    let (win, e) = ui::window_with("x");
    let s = status(&e);
    assert!(!s.is_shown());
    s.flash("hello");
    ui::pump(1.5);
    assert!(s.is_shown());
    assert_eq!(s.text(), "hello");
    s.flash("again"); // a new message restarts the clock
    ui::pump(1.5);
    assert_eq!(s.text(), "again");
    ui::pump(0.8);
    assert_eq!(s.text(), "");
    assert!(!s.is_shown());
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
    assert!((point_size(&e2) - 15.0 * 0.9).abs() < 1e-9);
    assert_eq!(status(&e1).text(), "Text size 90%");
    assert_eq!(status(&e2).text(), "");
    w1.close();
    w2.close();
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
    assert_eq!(point_size(&e), 15.0 * 1.5, "size comes from the zoom");
    panel.close();
    win.close();
}

// ---- External changes and autosave holds ----

/// A document opened from a file in a fresh temp dir, with a window, as the
/// document controller would open it (type, URL, modification date).
struct OnDisk {
    doc: Retained<PrasastiDocument>,
    win: Retained<objc2_app_kit::NSWindow>,
    path: std::path::PathBuf,
}

impl OnDisk {
    fn disk(&self) -> String {
        String::from_utf8(std::fs::read(&self.path).expect("file on disk")).unwrap()
    }
    fn write(&self, s: &str) {
        std::fs::write(&self.path, s).unwrap();
    }
    fn text(&self) -> String {
        self.doc.text().to_string()
    }
    /// The recovery draft AppKit restores after a crash, if one was written.
    fn draft(&self) -> Option<std::path::PathBuf> {
        Some(self.doc.autosavedContentsFileURL()?.path()?.to_string().into())
    }
    fn status(&self) -> String {
        self.doc.editor().unwrap().status().unwrap().text()
    }
    /// Types through the window and lets the run loop close the undo group.
    fn type_text(&self, s: &str) {
        ui::type_text(&self.win, s);
        ui::pump(0.05);
        assert!(self.doc.isDocumentEdited(), "typing marks the document edited");
    }
    fn autosave(&self, cancellable: bool) -> bool {
        let failed = std::rc::Rc::new(std::cell::Cell::new(None));
        let f = failed.clone();
        let done = block2::RcBlock::new(move |e: *mut NSError| f.set(Some(!e.is_null())));
        self.doc.autosaveWithImplicitCancellability_completionHandler(cancellable, &done);
        wait_until(|| failed.get().is_some());
        !failed.get().unwrap()
    }
    /// File > Save through the menu (⌘S), then waits for the write to land.
    fn save(&self) {
        ui::shortcut(&self.win, "s", CMD);
        wait_until(|| !self.doc.isDocumentEdited());
    }
}

/// Pumps the run loop until `f` holds; panics after 5 s.
fn wait_until(f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        if start.elapsed().as_secs_f64() > 5.0 {
            // Name any sheet in the way (an unexpected AppKit alert).
            for w in NSApplication::sharedApplication(mtm()).windows().iter() {
                if let Some(sheet) = w.attachedSheet().and_then(|s| s.contentView()) {
                    eprintln!("sheet open: {:?}", texts_in(&sheet));
                }
            }
            panic!("timed out waiting");
        }
        ui::pump(0.02);
    }
}

fn disk_date(path: &std::path::Path) -> Retained<objc2_foundation::NSDate> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
    let key = unsafe { objc2_foundation::NSURLContentModificationDateKey };
    let vals = url.resourceValuesForKeys_error(&objc2_foundation::NSArray::from_slice(&[key])).expect("date");
    vals.objectForKey(key).expect("date").downcast().expect("NSDate")
}

fn drafts_dir() -> std::path::PathBuf {
    std::env::home_dir().unwrap().join("Library/Autosave Information")
}

fn draft_names() -> std::collections::HashSet<std::ffi::OsString> {
    std::fs::read_dir(drafts_dir()).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default()
}

/// Removes the test documents' drafts created since `before` (never anyone else's).
fn remove_new_drafts(before: &std::collections::HashSet<std::ffi::OsString>) {
    for name in draft_names().difference(before) {
        let name_str = name.to_string_lossy();
        if ["doc.md (Autosaved ", "a-b.md (Autosaved "].iter().any(|p| name_str.starts_with(p)) {
            let _ = std::fs::remove_file(drafts_dir().join(name));
        }
    }
}

/// Runs `f` on a document opened from `initial`, sheets off; cleans up after.
fn on_disk(name: &str, initial: &str, f: impl FnOnce(&OnDisk)) {
    let dir = std::env::temp_dir().join(format!(
        "prasasti-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.md");
    std::fs::write(&path, initial).unwrap();
    let sheets = document::SHEETS_ENABLED.swap(false, std::sync::atomic::Ordering::SeqCst);
    let drafts_before = draft_names();
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
    let doc = PrasastiDocument::new(mtm());
    doc.readFromURL_ofType_error(&url, ns_string!(MD)).expect("read");
    doc.setFileURL(Some(&url));
    doc.setFileType(Some(ns_string!(MD)));
    doc.setFileModificationDate(Some(&disk_date(&path)));
    doc.makeWindowControllers();
    let win = doc.windowControllers().firstObject().and_then(|c| c.window()).expect("window");
    // Files open in Preview; these tests type.
    content_of(&win).set_mode(Mode::Edit);
    doc.editor().unwrap().setSelectedRange(NSRange::new(0, 0));
    let fx = OnDisk { doc, win, path };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&fx)));
    fx.doc.close();
    remove_new_drafts(&drafts_before);
    document::SHEETS_ENABLED.store(sheets, std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// External writes get a later mtime than the open, as real ones would.
fn later() {
    std::thread::sleep(std::time::Duration::from_millis(20));
}

fn own_write_ignored() {
    on_disk("own", "a\n", |fx| {
        fx.type_text("b");
        fx.save();
        assert_eq!(fx.disk(), "ba\n");
        fx.doc.disk_changed();
        assert_eq!(fx.doc.hold(), None);
        assert_eq!(fx.text(), "ba\n");
    });
}

fn clean_doc_reloads_silently() {
    on_disk("clean", "old text\n", |fx| {
        let editor = fx.doc.editor().unwrap();
        editor.setSelectedRange(NSRange::new(5, 3));
        later();
        fx.write("new\n");
        fx.doc.disk_changed();
        assert_eq!(fx.text(), "new\n");
        assert_eq!(fx.doc.hold(), None);
        assert!(!fx.doc.isDocumentEdited());
        // Clamped to the new length (4).
        assert_eq!(editor.selectedRange(), NSRange::new(4, 0));
    });
}

fn edited_doc_holds() {
    on_disk("edited", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
        assert_eq!(fx.status(), "Auto-save paused: file changed on disk");
        assert_eq!(fx.text(), "mine a\n", "local text kept");
        assert!(fx.doc.isDocumentEdited());
    });
}

fn autosave_skipped_while_held() {
    on_disk("autosave", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        assert!(fx.autosave(true), "an implicit autosave reports success");
        assert_eq!(fx.disk(), "theirs\n");
        let draft = fx.draft().expect("a recovery draft while held");
        assert!(draft.to_string_lossy().contains("/Autosave Information/"), "{draft:?}");
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), "mine a\n");
        // A forced autosave (closing, quitting) must fail so the window stays.
        fx.type_text("more ");
        assert!(!fx.autosave(false), "a forced autosave reports the hold");
        assert_eq!(fx.disk(), "theirs\n");
        let newer = fx.draft().expect("draft");
        assert_eq!(std::fs::read_to_string(&newer).unwrap(), "mine more a\n", "drafted first");
        assert_ne!(newer, draft, "each draft gets a fresh name");
        assert!(fx.doc.isDocumentEdited());
    });
}

fn autosave_runs_when_not_held() {
    on_disk("autosave-free", "a\n", |fx| {
        fx.type_text("b");
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "ba\n");
        // Our own write is not an external change.
        fx.doc.disk_changed();
        assert_eq!(fx.doc.hold(), None);
    });
}

fn keep_mine_then_save_overwrites() {
    on_disk("keep", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        fx.doc.resolve_external(false);
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange), "held until an explicit save");
        assert!(fx.doc.isDocumentEdited());
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "theirs\n", "autosave still held");
        let draft = fx.draft().expect("draft");
        fx.save();
        assert_eq!(fx.disk(), "mine a\n");
        assert_eq!(fx.doc.hold(), None);
        assert_eq!(fx.draft(), None, "Save forgets the draft");
        assert!(!draft.exists(), "Save deletes the draft");
        assert_eq!(fx.status(), "", "paused status clears with the hold");
    });
}

fn reload_discards() {
    on_disk("reload", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        assert!(fx.autosave(true));
        let draft = fx.draft().expect("draft");
        fx.doc.resolve_external(true);
        assert_eq!(fx.text(), "theirs\n");
        assert_eq!(fx.doc.hold(), None);
        assert_eq!(fx.status(), "");
        assert_eq!(fx.draft(), None, "Reload forgets the draft");
        assert!(!draft.exists(), "Reload deletes the draft");
        assert!(!fx.doc.isDocumentEdited());
    });
}

fn deleted_file_holds() {
    on_disk("deleted", "a\n", |fx| {
        fx.type_text("mine ");
        std::fs::remove_file(&fx.path).unwrap();
        fx.doc.disk_changed();
        assert_eq!(fx.doc.hold(), Some(HoldReason::Deleted));
        assert_eq!(fx.status(), "Auto-save paused: file deleted");
        assert!(fx.autosave(true));
        assert!(!fx.path.exists(), "autosave does not recreate the file");
        assert_eq!(std::fs::read_to_string(fx.draft().expect("draft")).unwrap(), "mine a\n");
        // ⌘S writes it back in place (not AppKit's "location cannot be determined").
        fx.save();
        assert_eq!(fx.disk(), "mine a\n");
        assert_eq!(fx.doc.hold(), None);
        assert_eq!(fx.text(), "mine a\n");
    });
}

objc2::define_class!(
    /// Records the answer of `canCloseDocumentWithDelegate:…`.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[ivars = std::cell::Cell<Option<bool>>]
    struct CloseProbe;

    impl CloseProbe {
        #[unsafe(method(document:shouldClose:contextInfo:))]
        fn should_close(&self, _doc: &NSDocument, should: objc2::runtime::Bool, _ctx: *mut std::ffi::c_void) {
            self.ivars().set(Some(should.as_bool()));
        }
    }
);

/// Closing a held, edited document must not report "can close" (AppKit would
/// drop the text typed since the hold) nor write the file.
fn close_while_held_keeps_document() {
    on_disk("close-held", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        // A draft written first must not make AppKit think nothing is left to save.
        assert!(fx.autosave(true));
        let probe: Retained<CloseProbe> = unsafe { objc2::msg_send![super(CloseProbe::alloc().set_ivars(Default::default())), init] };
        // SAFETY: the selector matches CloseProbe's method; no context.
        unsafe {
            fx.doc.canCloseDocumentWithDelegate_shouldCloseSelector_contextInfo(
                &probe,
                Some(objc2::sel!(document:shouldClose:contextInfo:)),
                std::ptr::null_mut(),
            )
        };
        // AppKit tells the user why, as a sheet; the answer comes once it is dismissed.
        click(&sheet_button(&fx.win, "OK"));
        wait_until(|| probe.ivars().get().is_some());
        assert_eq!(probe.ivars().get(), Some(false), "a held document refuses to close");
        assert_eq!(fx.disk(), "theirs\n");
        assert_eq!(fx.text(), "mine a\n");
    });
}

/// A document reopened from a recovery draft (as AppKit does after a crash),
/// with `original` on disk at its file URL (None: missing).
fn recovered(name: &str, original: Option<&[u8]>, f: impl FnOnce(&Retained<PrasastiDocument>, &std::path::Path)) {
    let dir = std::env::temp_dir().join(format!("prasasti-{name}-{}-{:?}", std::process::id(), Instant::now()).replace([' ', ':', '{', '}', ','], ""));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.md");
    if let Some(b) = original {
        std::fs::write(&path, b).unwrap();
    }
    let drafts_before = draft_names();
    let draft = dir.join("draft.md");
    std::fs::write(&draft, "mine\n").unwrap();
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
    let draft_url = NSURL::fileURLWithPath(&NSString::from_str(draft.to_str().unwrap()));
    // SAFETY: NSDocument's initializer for reopening autosaved contents.
    let doc: Result<Retained<PrasastiDocument>, Retained<NSError>> = unsafe {
        objc2::msg_send![<PrasastiDocument as objc2::MainThreadOnly>::alloc(mtm()), initForURL: Some(&*url), withContentsOfURL: &*draft_url, ofType: ns_string!(MD), error: _]
    };
    let doc = doc.expect("reopen from draft");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&doc, &path)));
    doc.close();
    remove_new_drafts(&drafts_before);
    let _ = std::fs::remove_dir_all(&dir);
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

fn autosave_doc(doc: &PrasastiDocument) {
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let d = done.clone();
    let block = block2::RcBlock::new(move |_e: *mut NSError| d.set(true));
    doc.autosaveWithImplicitCancellability_completionHandler(true, &block);
    wait_until(|| done.get());
}

/// Untitled-window restore and File > Duplicate: initForURL:nil withContentsOfURL:<draft>.
fn untitled_draft_has_no_hold() {
    let dir = std::env::temp_dir().join(format!("prasasti-untitled-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let draft = dir.join("draft.md");
    std::fs::write(&draft, "mine\n").unwrap();
    let draft_url = NSURL::fileURLWithPath(&NSString::from_str(draft.to_str().unwrap()));
    // SAFETY: NSDocument's initializer for reopening autosaved contents.
    let doc: Result<Retained<PrasastiDocument>, Retained<NSError>> = unsafe {
        objc2::msg_send![<PrasastiDocument as objc2::MainThreadOnly>::alloc(mtm()), initForURL: None::<&NSURL>, withContentsOfURL: &*draft_url, ofType: ns_string!(MD), error: _]
    };
    let doc = doc.expect("reopen untitled draft");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_eq!(doc.fileURL(), None);
        assert_eq!(doc.text().to_string(), "mine\n");
        assert_eq!(doc.hold(), None, "no recovery hold without a file");
        let failed = std::rc::Rc::new(std::cell::Cell::new(None));
        let f = failed.clone();
        let done = block2::RcBlock::new(move |e: *mut NSError| f.set(Some(!e.is_null())));
        doc.autosaveWithImplicitCancellability_completionHandler(true, &done);
        wait_until(|| failed.get().is_some());
        assert_eq!(failed.get(), Some(false), "autosave works normally");
    }));
    if let Some(p) = doc.autosavedContentsFileURL().and_then(|u| u.path()) {
        let p = std::path::PathBuf::from(p.to_string());
        if p.starts_with(drafts_dir()) {
            let _ = std::fs::remove_file(p);
        }
    }
    doc.close();
    let _ = std::fs::remove_dir_all(&dir);
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// ⌘N: the document controller's `initWithType:error:` must leave working ivars.
fn init_with_type_has_ivars() {
    // SAFETY: NSDocument's initializer for a new untitled document.
    let doc: Result<Retained<PrasastiDocument>, Retained<NSError>> = unsafe {
        objc2::msg_send![<PrasastiDocument as objc2::MainThreadOnly>::alloc(mtm()), initWithType: ns_string!(MD), error: _]
    };
    let doc = doc.expect("new document");
    assert_eq!(doc.hold(), None);
    assert_eq!(doc.text().to_string(), "");
    doc.readFromData_ofType_error(&NSData::with_bytes(b"typed\n"), ns_string!(MD)).expect("set text");
    assert_eq!(doc.text().to_string(), "typed\n");
    doc.close();
}

/// Open: the document controller's `initWithContentsOfURL:ofType:error:`.
fn init_with_contents_has_ivars() {
    on_disk("init-contents", "x", |fx| {
        let url = fx.doc.fileURL().unwrap();
        // SAFETY: NSDocument's initializer for opening a file.
        let doc: Result<Retained<PrasastiDocument>, Retained<NSError>> = unsafe {
            objc2::msg_send![<PrasastiDocument as objc2::MainThreadOnly>::alloc(mtm()), initWithContentsOfURL: &*url, ofType: ns_string!(MD), error: _]
        };
        let doc = doc.expect("open");
        assert_eq!(doc.text().to_string(), "x");
        assert_eq!(doc.hold(), None);
        assert!(!doc.isDocumentEdited());
        doc.readFromData_ofType_error(&NSData::with_bytes(b"a\r\nb"), ns_string!(MD)).expect("set text");
        assert_eq!(doc.text().to_string(), "a\nb");
        assert_eq!(doc.hold(), Some(HoldReason::LineEndings));
        doc.close();
    });
}

fn recovered_draft_keeps_hold() {
    let cases: [(&str, Option<&[u8]>, HoldReason); 3] = [
        ("rec-crlf", Some(b"a\r\nb"), HoldReason::LineEndings),
        ("rec-missing", None, HoldReason::Deleted),
        ("rec-changed", Some(b"theirs\n"), HoldReason::ExternalChange),
    ];
    for (name, original, want) in cases {
        recovered(name, original, |doc, path| {
            assert_eq!(doc.text().to_string(), "mine\n");
            assert_eq!(doc.hold(), Some(want), "{name}");
            assert!(doc.isDocumentEdited(), "{name}: recovered text is unsaved");
            // Our own bookkeeping must not call the original "our write".
            doc.disk_changed();
            assert_eq!(doc.hold(), Some(want), "{name}");
            autosave_doc(doc);
            assert_eq!(std::fs::read(path).ok().as_deref(), original, "{name}: original untouched");
        });
    }
}

fn drafts_rotate_keeping_two() {
    on_disk("rotate", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        let mut drafts = Vec::new();
        for _ in 0..3 {
            assert!(fx.autosave(true));
            drafts.push(fx.draft().expect("draft"));
        }
        assert!(!drafts[0].exists(), "oldest removed");
        assert!(drafts[1].exists(), "previous kept until the next rotation");
        assert!(drafts[2].exists(), "latest");
        fx.doc.resolve_external(false);
        fx.save();
        assert!(drafts.iter().all(|d| !d.exists()), "Save removes every draft");
    });
}

fn hold_change_replaces_sheet() {
    on_disk("sheet-swap", "a\n", |fx| {
        document::SHEETS_ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        let _ = sheet_button(&fx.win, "Reload");
        std::fs::remove_file(&fx.path).unwrap();
        fx.doc.disk_changed();
        wait_until(|| {
            fx.win.attachedSheet().and_then(|s| s.contentView()).is_some_and(|v| texts_in(&v).iter().any(|t| t == "Close"))
        });
        let texts = texts_in(&fx.win.attachedSheet().unwrap().contentView().unwrap());
        assert!(!texts.iter().any(|t| t == "Reload"), "stale sheet gone: {texts:?}");
        assert_eq!(fx.doc.hold(), Some(HoldReason::Deleted));
        assert_eq!(fx.text(), "mine a\n");
        click(&sheet_button(&fx.win, "Save"));
        wait_until(|| fx.path.exists() && !fx.doc.isDocumentEdited());
        assert_eq!(fx.disk(), "mine a\n");
    });
}

fn sheet_button(win: &objc2_app_kit::NSWindow, title: &str) -> Retained<NSButton> {
    wait_until(|| win.attachedSheet().is_some());
    let sheet = win.attachedSheet().unwrap().contentView().unwrap();
    buttons_in(&sheet).into_iter().find(|b| b.title().to_string() == title).unwrap_or_else(|| panic!("no {title:?} button"))
}

fn deleted_sheet_save_writes_back() {
    on_disk("deleted-save", "a\n", |fx| {
        document::SHEETS_ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
        fx.type_text("mine ");
        std::fs::remove_file(&fx.path).unwrap();
        fx.doc.disk_changed();
        let _ = sheet_button(&fx.win, "Close");
        click(&sheet_button(&fx.win, "Save"));
        wait_until(|| fx.path.exists() && !fx.doc.isDocumentEdited());
        assert_eq!(fx.disk(), "mine a\n");
        assert_eq!(fx.doc.hold(), None);
    });
}

fn removed_file_returns_changed() {
    on_disk("returns", "a\n", |fx| {
        std::fs::remove_file(&fx.path).unwrap();
        fx.doc.disk_changed();
        assert_eq!(fx.doc.hold(), Some(HoldReason::Deleted));
        later();
        fx.write("back\n");
        fx.doc.disk_changed();
        assert_eq!(fx.text(), "back\n", "a clean document reloads the new file");
        assert_eq!(fx.doc.hold(), None);
    });
    on_disk("returns-edited", "a\n", |fx| {
        fx.type_text("mine ");
        std::fs::remove_file(&fx.path).unwrap();
        fx.doc.disk_changed();
        later();
        fx.write("back\n");
        fx.doc.disk_changed();
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
        assert_eq!(fx.text(), "mine a\n");
    });
}

fn crlf_hold_blocks_autosave_until_save() {
    on_disk("crlf", "a\r\nb", |fx| {
        assert_eq!(fx.doc.hold(), Some(HoldReason::LineEndings));
        assert_eq!(fx.status(), "Auto-save paused: line endings");
        fx.type_text("x");
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "a\r\nb", "autosave never writes over CRs");
        fx.save();
        assert_eq!(fx.disk(), "xa\nb");
        assert_eq!(fx.doc.hold(), None);
        assert_eq!(fx.status(), "");
    });
}

fn paused_status_outlives_transient_status() {
    on_disk("status", "a\r\n", |fx| {
        let editor = fx.doc.editor().unwrap();
        editor.status().unwrap().flash("Text size 110%");
        ui::pump(2.3);
        assert_eq!(fx.status(), "Auto-save paused: line endings", "back after the 2 s message, and stays");
    });
}

fn rename_follows_file() {
    on_disk("rename", "a\n", |fx| {
        let new = fx.path.with_file_name("renamed.md");
        std::fs::rename(&fx.path, &new).unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(new.to_str().unwrap()));
        fx.doc.presentedItemDidMoveToURL(&url);
        wait_until(|| fx.doc.fileURL().and_then(|u| u.path()).is_some_and(|p| p.to_string().ends_with("renamed.md")));
        fx.type_text("b");
        assert!(fx.autosave(true));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "ba\n");
        assert!(!fx.path.exists(), "nothing written at the old name");
    });
}

fn sheet_keep_mine_button() {
    on_disk("sheet", "a\n", |fx| {
        document::SHEETS_ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        let _ = sheet_button(&fx.win, "Reload");
        click(&sheet_button(&fx.win, "Keep Mine"));
        wait_until(|| fx.win.attachedSheet().is_none());
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
        assert_eq!(fx.text(), "mine a\n");
        fx.save();
        assert_eq!(fx.disk(), "mine a\n");
    });
}

fn texts_in(v: &objc2_app_kit::NSView) -> Vec<String> {
    let mut out = Vec::new();
    for sub in v.subviews().iter() {
        if let Ok(t) = sub.clone().downcast::<objc2_app_kit::NSTextField>() {
            out.push(t.stringValue().to_string());
        }
        if let Ok(b) = sub.clone().downcast::<NSButton>() {
            out.push(b.title().to_string());
        }
        out.extend(texts_in(&sub));
    }
    out
}

fn buttons_in(v: &objc2_app_kit::NSView) -> Vec<Retained<NSButton>> {
    let mut out = Vec::new();
    for sub in v.subviews().iter() {
        if let Ok(b) = sub.clone().downcast::<NSButton>() {
            out.push(b);
        }
        out.extend(buttons_in(&sub));
    }
    out
}

// ---- Runtime class names and typing around autosave ----

/// The class the Objective-C runtime knows by `name`, if any.
fn class_named(name: &str) -> Option<&'static objc2::runtime::AnyClass> {
    objc2::runtime::AnyClass::get(&std::ffi::CString::new(name).unwrap())
}

fn document_class_registered_by_name() {
    use objc2::ClassType;
    let ours = PrasastiDocument::class();
    assert_eq!(class_named("PrasastiDocument"), Some(ours));
}

/// NSDocumentController finds the document class by the name in Info.plist.
fn info_plist_classes_resolve() {
    use objc2::ClassType;
    let _ = PrasastiDocument::class();
    let plist = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/Info.plist");
    for key in ["NSPrincipalClass", "CFBundleDocumentTypes.0.NSDocumentClass"] {
        let out = std::process::Command::new("plutil").args(["-extract", key, "raw", plist]).output().unwrap();
        assert!(out.status.success(), "{key} missing from Info.plist");
        let name = String::from_utf8(out.stdout).unwrap();
        assert!(class_named(name.trim()).is_some(), "{key} = {name:?} is not a registered class");
    }
}

/// Typing after an in-place autosave is a new change: edited, and autosaved next time.
fn typing_after_autosave_counts() {
    on_disk("after-autosave", "a\n", |fx| {
        fx.type_text("b");
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "ba\n");
        fx.type_text("c");
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "bca\n");
    });
}

/// Typing after the autosave took its snapshot, before it finished, still
/// counts as a change (the smoke run lost such an edit).
fn typing_during_autosave_counts() {
    on_disk("during-autosave", "a\n", |fx| {
        fx.type_text("b");
        let win = fx.win.clone();
        document::AFTER_SNAPSHOT.with(|h| *h.borrow_mut() = Some(Box::new(move || ui::type_text(&win, "c"))));
        assert!(fx.autosave(true));
        assert!(document::AFTER_SNAPSHOT.with(|h| h.borrow().is_none()), "the hook ran");
        assert_eq!(fx.disk(), "ba\n", "c came after the snapshot");
        ui::pump(0.05);
        assert!(fx.doc.isDocumentEdited(), "typing during the autosave marks the document edited");
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "bca\n");
    });
}

// ---- Disk checked before in-place writes (external writes the presenter missed) ----

fn autosave_checks_disk_first() {
    on_disk("autosave-check", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n"); // no presenter event, no disk_changed
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "theirs\n", "the external text is not overwritten");
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
        assert_eq!(std::fs::read_to_string(fx.draft().expect("draft")).unwrap(), "mine a\n");
    });
}

fn autosave_checks_missing_file() {
    on_disk("autosave-missing", "a\n", |fx| {
        fx.type_text("mine ");
        std::fs::remove_file(&fx.path).unwrap();
        assert!(fx.autosave(true));
        assert!(!fx.path.exists(), "autosave does not recreate the file");
        assert_eq!(fx.doc.hold(), Some(HoldReason::Deleted));
    });
}

fn save_checks_disk_first() {
    on_disk("save-check", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        ui::shortcut(&fx.win, "s", CMD);
        ui::pump(0.2);
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
        assert_eq!(fx.disk(), "theirs\n", "⌘S asks first instead of writing");
        assert!(fx.doc.isDocumentEdited());
        // Keep Mine, then Save, overwrites deliberately.
        fx.doc.resolve_external(false);
        fx.save();
        assert_eq!(fx.disk(), "mine a\n");
    });
}

fn rechecks_disk_after_save() {
    on_disk("recheck", "a\n", |fx| {
        fx.type_text("b");
        assert!(fx.autosave(true));
        fx.type_text("c");
        later();
        fx.write("theirs\n"); // lands just after ours; the presenter misses it
        assert_eq!(fx.doc.hold(), None);
        ui::pump(1.3);
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange), "noticed about 1 s after the save");
        assert_eq!(fx.text(), "bca\n");
    });
}

/// The close/quit dialog's Save (saveDocumentWithDelegate:) checks the disk
/// too: our hold, no write, no AppKit alert.
fn close_dialog_save_checks_disk() {
    on_disk("close-save-check", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        let doc: &NSDocument = &fx.doc;
        unsafe { doc.saveDocumentWithDelegate_didSaveSelector_contextInfo(None, None, std::ptr::null_mut()) };
        ui::pump(0.3);
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
        assert_eq!(fx.disk(), "theirs\n");
        assert!(fx.win.attachedSheet().is_none(), "no AppKit alert");
        assert!(fx.doc.isDocumentEdited());
    });
}

/// A plain Save that finds the disk changed reports user-cancelled, once.
fn save_op_cancels_when_disk_changed() {
    on_disk("save-op-check", "a\n", |fx| {
        fx.type_text("mine ");
        later();
        fx.write("theirs\n");
        let codes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let c = codes.clone();
        let done = block2::RcBlock::new(move |e: *mut NSError| c.borrow_mut().push(unsafe { e.as_ref() }.map(|e| e.code())));
        let url = fx.doc.fileURL().unwrap();
        fx.doc.saveToURL_ofType_forSaveOperation_completionHandler(&url, ns_string!(MD), objc2_app_kit::NSSaveOperationType::SaveOperation, &done);
        ui::pump(0.3);
        assert_eq!(*codes.borrow(), vec![Some(3072)], "NSUserCancelledError, exactly once");
        assert_eq!(fx.disk(), "theirs\n");
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
    });
}

/// After Keep Mine, the post-save re-check does not ask again.
fn recheck_after_keep_mine_stays_quiet() {
    on_disk("recheck-keep", "a\n", |fx| {
        document::SHEETS_ENABLED.store(true, std::sync::atomic::Ordering::SeqCst);
        fx.type_text("b");
        assert!(fx.autosave(true)); // schedules the re-check
        fx.type_text("c");
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        click(&sheet_button(&fx.win, "Keep Mine"));
        wait_until(|| fx.win.attachedSheet().is_none());
        ui::pump(1.3);
        assert!(fx.win.attachedSheet().is_none(), "no second sheet");
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
    });
}

/// Through the real presenter path: a coordinated write from another thread
/// reaches `presentedItemDidChange` on the presenter queue, then main.
fn presenter_reloads_clean_doc() {
    on_disk("presenter", "a\n", |fx| {
        // The document controller registers it as a file presenter, as in the app.
        objc2_app_kit::NSDocumentController::sharedDocumentController(mtm()).addDocument(&fx.doc);
        let path = fx.path.clone();
        later();
        std::thread::spawn(move || {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
            let c = objc2_foundation::NSFileCoordinator::initWithFilePresenter(objc2_foundation::NSFileCoordinator::alloc(), None);
            let block = block2::RcBlock::new(move |u: std::ptr::NonNull<NSURL>| {
                let p = unsafe { u.as_ref() }.path().unwrap().to_string();
                std::fs::write(p, "from elsewhere\n").unwrap();
            });
            c.coordinateWritingItemAtURL_options_error_byAccessor(
                &url,
                objc2_foundation::NSFileCoordinatorWritingOptions::empty(),
                None,
                &block,
            );
        })
        .join()
        .unwrap();
        let start = Instant::now();
        while fx.text() != "from elsewhere\n" && start.elapsed().as_secs_f64() < 5.0 {
            ui::pump(0.05);
        }
        assert_eq!(fx.text(), "from elsewhere\n");
        assert_eq!(fx.doc.hold(), None);
    });
}

/// Runs `f` on a document opened from `name` holding `initial`, as in the
/// app: opened from its file and owned by the document controller. Sheets off; cleans up after.
fn controller_doc(name: &str, initial: &[u8], f: impl FnOnce(&PrasastiDocument, &objc2_app_kit::NSWindow, &std::path::Path)) {
    let dir = std::env::temp_dir().join(format!("prasasti-held-ctl-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, initial).unwrap();
    let sheets = document::SHEETS_ENABLED.swap(false, std::sync::atomic::Ordering::SeqCst);
    let drafts_before = draft_names();
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
    // SAFETY: NSDocument's initializer for opening a file, as the controller calls it.
    let doc: Result<Retained<PrasastiDocument>, Retained<NSError>> = unsafe {
        objc2::msg_send![<PrasastiDocument as objc2::MainThreadOnly>::alloc(mtm()), initWithContentsOfURL: &*url, ofType: ns_string!(MD), error: _]
    };
    let doc = doc.expect("open");
    let dc = objc2_app_kit::NSDocumentController::sharedDocumentController(mtm());
    dc.addDocument(&doc);
    doc.makeWindowControllers();
    let win = doc.windowControllers().firstObject().and_then(|c| c.window()).expect("window");
    // Files open in Preview; these tests type.
    content_of(&win).set_mode(Mode::Edit);
    doc.editor().unwrap().setSelectedRange(NSRange::new(0, 0));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&doc, &win, &path)));
    doc.close();
    remove_new_drafts(&drafts_before);
    document::SHEETS_ENABLED.store(sheets, std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// The completion error's description, once the handler has run (None = success).
type Outcome = std::rc::Rc<std::cell::RefCell<Option<Option<String>>>>;

fn completion() -> (Outcome, block2::RcBlock<dyn Fn(*mut NSError)>) {
    let err = std::rc::Rc::new(std::cell::RefCell::new(None));
    let e2 = err.clone();
    let done = block2::RcBlock::new(move |e: *mut NSError| {
        *e2.borrow_mut() = Some(unsafe { e.as_ref() }.map(|e| format!("{:?}", e.description())));
    });
    (err, done)
}

fn draft_text(doc: &PrasastiDocument) -> String {
    let draft: std::path::PathBuf = doc.autosavedContentsFileURL().and_then(|u| u.path()).expect("a recovery draft").to_string().into();
    std::fs::read_to_string(&draft).unwrap()
}

/// A held document must still write a recovery draft holding the local text.
fn held_controller_doc_writes_draft() {
    controller_doc("doc.md", b"a\r\nb", |doc, win, path| {
        assert_eq!(doc.hold(), Some(HoldReason::LineEndings));
        ui::type_text(win, "mine ");
        ui::pump(0.05);
        let (err, done) = completion();
        doc.autosaveWithImplicitCancellability_completionHandler(true, &done);
        wait_until(|| err.borrow().is_some());
        assert_eq!(err.borrow().clone().unwrap(), None, "draft write failed");
        assert_eq!(draft_text(doc), "mine a\nb");
        assert_eq!(std::fs::read(path).unwrap(), b"a\r\nb", "the original is untouched");
    });
}

/// AppKit itself asks for an autosave-elsewhere ("Ask to keep changes when
/// closing documents", locked files); for a file-backed document it must
/// write a draft too, not fail with 512.
fn controller_doc_elsewhere_op_writes_draft() {
    controller_doc("doc.md", b"a\r\nb", |doc, win, path| {
        ui::type_text(win, "mine ");
        ui::pump(0.05);
        let (err, done) = completion();
        let name = format!("doc.md (Autosaved {}).md", objc2_foundation::NSProcessInfo::processInfo().globallyUniqueString());
        let dir = NSURL::fileURLWithPath(&NSString::from_str(drafts_dir().to_str().unwrap()));
        let url = dir.URLByAppendingPathComponent(&NSString::from_str(&name)).unwrap();
        doc.saveToURL_ofType_forSaveOperation_completionHandler(&url, ns_string!(MD), objc2_app_kit::NSSaveOperationType::AutosaveElsewhereOperation, &done);
        wait_until(|| err.borrow().is_some());
        assert_eq!(err.borrow().clone().unwrap(), None, "elsewhere save failed");
        assert_eq!(draft_text(doc), "mine a\nb");
        assert_eq!(std::fs::read(path).unwrap(), b"a\r\nb", "the original is untouched");
    });
}

/// `a:b.md` shows as `a/b.md`; the draft name must still be one path component.
fn draft_name_with_slash_is_written() {
    controller_doc("a:b.md", b"a\r\nb", |doc, win, _| {
        ui::type_text(win, "mine ");
        ui::pump(0.05);
        let (err, done) = completion();
        doc.autosaveWithImplicitCancellability_completionHandler(true, &done);
        wait_until(|| err.borrow().is_some());
        assert_eq!(err.borrow().clone().unwrap(), None, "draft write failed");
        let draft = doc.autosavedContentsFileURL().and_then(|u| u.path()).expect("a draft").to_string();
        assert_eq!(std::path::Path::new(&draft).parent(), Some(drafts_dir().as_path()));
        assert_eq!(draft_text(doc), "mine a\nb");
    });
}

/// A failed save leaves no stale snapshot (⌘S would skip its disk check).
fn failed_save_clears_snapshot() {
    on_disk("failed-save", "a\n", |fx| {
        fx.type_text("mine ");
        let (err, done) = completion();
        // A read-only folder: the data is produced, then the write fails.
        let ro = fx.path.parent().unwrap().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
        let nowhere = NSURL::fileURLWithPath(&NSString::from_str(ro.join("x.md").to_str().unwrap()));
        fx.doc.saveToURL_ofType_forSaveOperation_completionHandler(&nowhere, ns_string!(MD), objc2_app_kit::NSSaveOperationType::SaveToOperation, &done);
        wait_until(|| err.borrow().is_some());
        let failed = err.borrow().clone().unwrap().is_some();
        std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(failed, "the save must fail");
        assert!(!fx.doc.writing(), "no save in flight");
    });
}

/// Duplicate leaves no stale snapshot either.
fn duplicate_clears_snapshot() {
    on_disk("dup-save", "a\n", |fx| {
        fx.type_text("mine ");
        let doc: &NSDocument = &fx.doc;
        // The test bundle declares no document types, so opening the copy
        // fails; the copy's data is written first, as in the app.
        if let Ok(copy) = doc.duplicateAndReturnError() {
            copy.close();
        }
        assert!(!fx.doc.writing(), "no save in flight");
    });
}

/// The post-save re-check does nothing for a document closed meanwhile.
fn recheck_skips_closed_document() {
    on_disk("recheck-closed", "a\n", |fx| {
        fx.type_text("b");
        assert!(fx.autosave(true)); // schedules the re-check
        fx.type_text("c");
        fx.doc.close();
        later();
        fx.write("theirs\n");
        ui::pump(1.3);
        assert_eq!(fx.doc.hold(), None);
    });
}

fn glass_and_webkit_available() {
    use objc2::runtime::AnyClass;
    use objc2_app_kit::NSTextField;
    assert!(AnyClass::get(c"NSGlassEffectView").is_some());
    let glass = prasasti::glass::GlassView::new(mtm());
    let field = NSTextField::new(mtm());
    glass.setContentView(Some(&field));
    assert!(glass.contentView().is_some_and(|v| Retained::as_ptr(&v) == Retained::as_ptr(&field).cast()));
    let web = unsafe {
        objc2_web_kit::WKWebView::initWithFrame_configuration(
            mtm().alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 100.0)),
            &objc2_web_kit::WKWebViewConfiguration::new(mtm()),
        )
    };
    drop(web);
}

// The fonts are only installed inside the bundle; register them for this process.
fn register_bundled_fonts() {
    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        // `url` is a CFURLRef; NSURL is toll-free bridged to it.
        fn CTFontManagerRegisterFontsForURL(url: *const AnyObject, scope: u32, error: *mut *const AnyObject) -> bool;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/fonts");
    for entry in std::fs::read_dir(dir).expect("resources/fonts") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "ttf") {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().expect("utf-8 path")));
            // SAFETY: a valid file URL; scope 1 is kCTFontManagerScopeProcess; the error out-pointer may be null.
            assert!(unsafe { CTFontManagerRegisterFontsForURL(&*url as *const NSURL as *const AnyObject, 1, std::ptr::null_mut()) }, "{path:?}");
        }
    }
}

/// The checked-in kitchen-sink fixture is valid UTF-8/LF, has every section, its
/// local images exist, and its assets stay small.
fn kitchen_sink_is_well_formed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/kitchen-sink");
    let bytes = std::fs::read(dir.join("kitchen-sink.md")).expect("fixture exists");
    assert!(!bytes.contains(&b'\r'), "fixture must be LF only");
    let text = String::from_utf8(bytes).expect("fixture is UTF-8");
    for h in [
        "Headings", "Paragraphs", "Links", "Lists", "Block quotes", "Code", "Tables", "Footnotes", "Math", "Mermaid",
        "Images", "Raw HTML", "Non-ASCII",
    ] {
        assert!(text.lines().any(|l| l == format!("## {h}")), "missing h2 {h}");
    }
    let u16s: Vec<u16> = text.encode_utf16().collect();
    let dests = prasasti_core::images::image_destinations(&u16s);
    let mut local = 0;
    for d in &dests {
        let p = String::from_utf16(&d.path).unwrap();
        if p.contains("://") || p.starts_with("data:") || p == "assets/missing.png" {
            continue;
        }
        assert!(dir.join(&p).is_file(), "missing image {p}");
        local += 1;
    }
    assert_eq!(local, 4, "four local images");
    let (mut total, mut stack) = (0, vec![dir.join("assets")]);
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).expect("assets dir") {
            let e = e.unwrap();
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                let n = e.metadata().unwrap().len();
                assert!(n < 100_000, "{:?} is {n} bytes", e.path());
                total += n;
            }
        }
    }
    assert!(total < 500_000, "assets total {total} bytes");
}

// ---- Chrome, status pill, fixed column ----

fn hold_shows_status_pill() {
    on_disk("pill", "a\r\nb", |fx| {
        let pill = fx.doc.editor().unwrap().status().unwrap();
        assert!(pill.is_shown());
        assert_eq!(pill.text(), "Auto-save paused: line endings");
        fx.type_text("x");
        fx.save();
        assert_eq!(fx.doc.hold(), None);
        assert!(!pill.is_shown());
    });
}

fn window_has_no_visible_titlebar() {
    on_disk("titlebar", "a\n", |fx| {
        // Opaque: AppKit blurs the text scrolling under it only then.
        assert!(!fx.win.titlebarAppearsTransparent());
        assert_eq!(fx.win.titleVisibility(), objc2_app_kit::NSWindowTitleVisibility::Hidden);
        assert!(fx.win.styleMask().contains(objc2_app_kit::NSWindowStyleMask::FullSizeContentView));
        assert!(fx.win.title().to_string().starts_with("doc"), "{}", fx.win.title());
    });
}

/// The width of `columnWidth` zeros in the editor font, plus the padding on both sides.
fn expected_column(e: &EditorView) -> f64 {
    let attrs = objc2_foundation::NSDictionary::from_slices(&[unsafe { objc2_app_kit::NSFontAttributeName }], &[&*settings::current_font() as &AnyObject]);
    let zeros = NSString::from_str(&"0".repeat(settings::column_width()));
    // SAFETY: the font attribute's value is a font.
    let w = unsafe { objc2_app_kit::NSStringDrawing::sizeWithAttributes(&*zeros, Some(&attrs)) }.width;
    w + 2.0 * unsafe { e.textContainer() }.unwrap().lineFragmentPadding()
}

fn container_width(e: &EditorView) -> f64 {
    unsafe { e.textContainer() }.unwrap().size().width
}

fn set_content_width(win: &objc2_app_kit::NSWindow, w: f64) {
    let f = win.frame();
    let content = win.contentRectForFrameRect(f);
    win.setFrame_display(win.frameRectForContentRect(NSRect::new(content.origin, NSSize::new(w, content.size.height))), false);
    ui::pump(0.05);
}

fn column_width_fixed() {
    fresh_defaults();
    let (win, e) = ui::window_with("x");
    set_content_width(&win, 900.0);
    let a = container_width(&e);
    set_content_width(&win, 1600.0);
    let b = container_width(&e);
    assert_eq!(a, b);
    assert!((a - expected_column(&e)).abs() <= 0.5, "{a} vs {}", expected_column(&e));
    // Centred: equal space either side of the column.
    let inset = e.textContainerInset().width;
    let clip = e.enclosingScrollView().unwrap().contentSize().width;
    assert!((inset * 2.0 + b - clip).abs() <= 1.0, "inset {inset}, clip {clip}");
    win.close();
}

fn window_min_width_follows_column() {
    fresh_defaults();
    let (win, e) = ui::window_with("x");
    let min = win.contentMinSize().width;
    assert!(min >= container_width(&e) + 32.0, "{min}");
    set_content_width(&win, min);
    let before = win.frame().size.width;
    ui::shortcut(&win, "=", CMD);
    ui::pump(0.05);
    assert!(win.frame().size.width > before, "{} vs {before}", win.frame().size.width);
    assert!(win.contentMinSize().width > min);
    win.close();
    fresh_defaults();
}

const FRAME_KEY: &str = "NSWindow Frame PrasastiMain";

/// Closes every open document so each frame test starts with no document window.
fn close_all_documents() {
    for d in objc2_app_kit::NSDocumentController::sharedDocumentController(mtm()).documents().iter() {
        d.close();
    }
    for w in NSApplication::sharedApplication(mtm()).windows().iter() {
        if w.windowController().and_then(|c| unsafe { c.document() }).is_some() {
            w.close();
        }
    }
}

fn document_window(doc: &PrasastiDocument) -> Retained<objc2_app_kit::NSWindow> {
    doc.windowControllers().firstObject().and_then(|c| c.window()).expect("window")
}

fn disk_doc(path: &std::path::Path) -> Retained<PrasastiDocument> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
    let doc = PrasastiDocument::new(mtm());
    doc.readFromURL_ofType_error(&url, ns_string!(MD)).expect("read");
    doc.setFileURL(Some(&url));
    doc.setFileType(Some(ns_string!(MD)));
    doc
}

fn assert_frames_equal(a: NSRect, b: NSRect) {
    let close = |x: f64, y: f64| (x - y).abs() <= 0.5;
    assert!(
        close(a.origin.x, b.origin.x) && close(a.origin.y, b.origin.y) && close(a.size.width, b.size.width) && close(a.size.height, b.size.height),
        "{a:?} != {b:?}"
    );
}

/// A frame well inside the main screen, above the column minimum, unlike the default.
fn distinctive_frame() -> NSRect {
    let v = objc2_app_kit::NSScreen::mainScreen(mtm()).expect("screen").visibleFrame();
    NSRect::new(NSPoint::new(v.origin.x + 60.0, v.origin.y + 40.0), NSSize::new(v.size.width.min(1400.0) - 300.0, 520.0))
}

/// Opens `doc`'s window, returns its frame, sets `next`, and closes the
/// document, as a user closing the window does. The pool drain releases the
/// window, which frees its autosave name for the next open.
fn open_resize_close(doc: Retained<PrasastiDocument>, next: NSRect) -> NSRect {
    autoreleasepool(move |_| {
        doc.makeWindowControllers();
        let win = document_window(&doc);
        // The first window must not be shifted by AppKit's cascade.
        assert!(!doc.windowControllers().firstObject().expect("controller").shouldCascadeWindows());
        let opened = win.frame();
        win.setFrame_display(next, false);
        doc.close();
        opened
    })
}

fn frame_restored_exactly() {
    fresh_defaults();
    close_all_documents();
    let want = distinctive_frame();
    let want2 = NSRect::new(NSPoint::new(want.origin.x + 30.0, want.origin.y + 20.0), NSSize::new(want.size.width - 40.0, 480.0));
    // Untitled: set a frame, close, reopen.
    open_resize_close(doc_from(b"one"), want);
    assert_frames_equal(open_resize_close(doc_from(b"two"), want2), want);
    // File-backed: opened from disk.
    let dir = std::env::temp_dir().join(format!("prasasti-frame-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.md");
    std::fs::write(&path, "disk").unwrap();
    let opened = open_resize_close(disk_doc(&path), want);
    let _ = std::fs::remove_dir_all(&dir);
    assert_frames_equal(opened, want2);
}

fn second_window_cascades() {
    fresh_defaults();
    close_all_documents();
    let a = doc_from(b"a");
    a.makeWindowControllers();
    let b = doc_from(b"b");
    b.makeWindowControllers();
    let (fa, fb) = (document_window(&a).frame(), document_window(&b).frame());
    assert!(fa.origin.x != fb.origin.x || fa.origin.y != fb.origin.y, "{fa:?} == {fb:?}");
    a.close();
    b.close();
}

fn last_closed_window_frame_is_stored() {
    fresh_defaults();
    close_all_documents();
    // Differs from the frame earlier tests left saved, which A would restore.
    let d = distinctive_frame();
    let want = NSRect::new(NSPoint::new(d.origin.x + 50.0, d.origin.y + 30.0), NSSize::new(d.size.width - 100.0, 450.0));
    // A and B overlap; A closes first, then B is moved and closed: B's frame wins.
    autoreleasepool(|_| {
        let a = doc_from(b"a");
        a.makeWindowControllers();
        let b = doc_from(b"b");
        b.makeWindowControllers();
        a.close();
        document_window(&b).setFrame_display(want, false);
        b.close();
    });
    assert_frames_equal(open_resize_close(doc_from(b"c"), want), want);
}

fn resize_in_place_is_stored_without_closing() {
    fresh_defaults();
    close_all_documents();
    autoreleasepool(|_| {
        let d = doc_from(b"x");
        d.makeWindowControllers();
        let win = document_window(&d);
        let mut f = win.frame();
        f.size.width += 37.0;
        f.size.height += 23.0;
        win.setFrame_display(f, false); // same origin, new size, no live resize
        let saved = NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str(FRAME_KEY)).expect("saved").to_string();
        let n: Vec<f64> = saved.split_whitespace().take(4).map(|v| v.parse().unwrap()).collect();
        assert_eq!((n[2], n[3]), (win.frame().size.width, win.frame().size.height), "{saved}");
        d.close();
    });
}

fn no_window_tabbing() {
    let d = doc_from(b"x");
    d.makeWindowControllers();
    assert!(!objc2_app_kit::NSWindow::allowsAutomaticWindowTabbing(mtm()));
    assert_eq!(document_window(&d).tabbingMode(), objc2_app_kit::NSWindowTabbingMode::Disallowed);
    d.close();
}

fn restored_narrow_window_grows() {
    fresh_defaults();
    close_all_documents();
    // A frame saved at 300 pt wide: the real restore path must grow it to the column.
    let v = objc2_app_kit::NSScreen::mainScreen(mtm()).expect("screen").visibleFrame();
    let d = doc_from(b"x");
    d.makeWindowControllers();
    let win = document_window(&d);
    win.setFrame_display(NSRect::new(NSPoint::new(v.origin.x + 40.0, v.origin.y + 40.0), NSSize::new(300.0, 400.0)), false);
    win.saveFrameUsingName(ns_string!("PrasastiMain"));
    d.close();
    let d = doc_from(b"y");
    d.makeWindowControllers();
    let win = document_window(&d);
    let content = win.contentRectForFrameRect(win.frame()).size.width;
    assert!(win.contentMinSize().width > 300.0);
    assert!(content >= win.contentMinSize().width, "{content} < {}", win.contentMinSize().width);
    d.close();
}

fn kitchen_sink_lines_fit_column() {
    fresh_defaults();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/kitchen-sink/kitchen-sink.md");
    let text = std::fs::read_to_string(path).expect("fixture");
    let (win, e) = ui::window_with(&text);
    content_of(&win).set_mode(Mode::Edit);
    // SAFETY: plain getters of the editor's TextKit 1 chain.
    let (lm, tc) = unsafe { (e.layoutManager().unwrap(), e.textContainer().unwrap()) };
    lm.ensureLayoutForTextContainer(&tc);
    let width = tc.size().width;
    // Fixed column (80 characters of the editor font), not tracking the view.
    let want = e.column_pts();
    assert!((width - want).abs() <= 0.5, "container {width} != column {want}");
    let glyphs = lm.numberOfGlyphs();
    let (mut i, mut lines) = (0, 0);
    while i < glyphs {
        let mut r = NSRange::new(0, 0);
        // SAFETY: `r` is a valid out pointer; `i` is a glyph index.
        let used = unsafe { lm.lineFragmentUsedRectForGlyphAtIndex_effectiveRange(i, &mut r) };
        assert!(used.size.width <= width + 0.01, "line at glyph {i}: {} > {width}", used.size.width);
        i = r.end().max(i + 1);
        lines += 1;
    }
    assert!(lines > 100, "{lines} lines");
    win.close();
}

// ---- Image paths on save ----

/// `saveToURL:ofType:forSaveOperation:` to `path`; waits and asserts success.
fn save_op(doc: &PrasastiDocument, path: &std::path::Path, op: objc2_app_kit::NSSaveOperationType) {
    assert!(try_save_op(doc, path, op), "save succeeds");
}

/// `saveToURL:ofType:forSaveOperation:` to `path`; waits, true if it succeeded.
fn try_save_op(doc: &PrasastiDocument, path: &std::path::Path, op: objc2_app_kit::NSSaveOperationType) -> bool {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
    let failed = std::rc::Rc::new(std::cell::Cell::new(None));
    let f = failed.clone();
    let done = block2::RcBlock::new(move |e: *mut NSError| f.set(Some(!e.is_null())));
    doc.saveToURL_ofType_forSaveOperation_completionHandler(&url, ns_string!(MD), op, &done);
    wait_until(|| failed.get().is_some());
    failed.get() == Some(false)
}

fn first_save_rebases_images() {
    let dir = std::env::temp_dir().join(format!("prasasti-rebase-first-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("img.png"), b"png").unwrap();
    let doc = PrasastiDocument::new(mtm());
    doc.makeWindowControllers();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let e = doc.editor().unwrap();
        let before = format!("hi\n![]({}/img.png)\n", dir.display());
        e.setString(&NSString::from_str(&before));
        e.setSelectedRange(NSRange::new(1, 0));
        save_op(&doc, &dir.join("doc.md"), objc2_app_kit::NSSaveOperationType::SaveOperation);
        assert_eq!(doc.text().to_string(), "hi\n![](img.png)\n");
        assert_eq!(std::fs::read_to_string(dir.join("doc.md")).unwrap(), "hi\n![](img.png)\n");
        assert_eq!(e.selectedRange(), NSRange::new(1, 0), "the caret before the image stays put");
        ui::pump(0.05);
        assert!(!doc.isDocumentEdited(), "the rebase is part of what was saved");
        assert_eq!(e.undoManager().unwrap().undoMenuItemTitle().to_string(), "Undo Update Image Paths");
        ui::shortcut(&document_window(&doc), "z", CMD);
        assert_eq!(doc.text().to_string(), before);
    }));
    doc.close();
    let _ = std::fs::remove_dir_all(&dir);
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// Save As into `<dir>/b/new.md` from `<dir>/doc.md` holding `![](doc.assets/p.png)`.
fn save_as_with_asset(fx: &OnDisk) -> std::path::PathBuf {
    let dir = fx.path.parent().unwrap();
    std::fs::create_dir_all(dir.join("doc.assets")).unwrap();
    std::fs::write(dir.join("doc.assets/p.png"), b"mine").unwrap();
    fx.doc.editor().unwrap().setString(ns_string!("![](doc.assets/p.png)\n"));
    let b = dir.join("b");
    std::fs::create_dir_all(&b).unwrap();
    save_op(&fx.doc, &b.join("new.md"), objc2_app_kit::NSSaveOperationType::SaveAsOperation);
    b
}

fn save_as_moves_relative_and_carries_assets() {
    on_disk("save-as-carry", "a\n", |fx| {
        let b = save_as_with_asset(fx);
        assert_eq!(std::fs::read(b.join("new.assets/p.png")).unwrap(), b"mine");
        assert_eq!(fx.text(), "![](new.assets/p.png)\n");
        assert_eq!(std::fs::read_to_string(b.join("new.md")).unwrap(), "![](new.assets/p.png)\n");
        assert!(fx.path.parent().unwrap().join("doc.assets/p.png").exists(), "copied, not moved");
    });
}

fn carry_never_overwrites() {
    on_disk("carry-clash", "a\n", |fx| {
        let b = fx.path.parent().unwrap().join("b");
        std::fs::create_dir_all(b.join("new.assets")).unwrap();
        std::fs::write(b.join("new.assets/p.png"), b"other").unwrap();
        save_as_with_asset(fx);
        assert_eq!(std::fs::read(b.join("new.assets/p.png")).unwrap(), b"other");
        assert_eq!(std::fs::read(b.join("new.assets/p-2.png")).unwrap(), b"mine");
        assert_eq!(fx.text(), "![](new.assets/p-2.png)\n");
    });
}

/// Typing right after a rebasing save completes, before the deferred clear runs:
/// the document stays edited.
fn typing_after_rebasing_save_stays_edited() {
    on_disk("rebase-typing", "a\n", |fx| {
        let e = fx.doc.editor().unwrap();
        e.setString(&NSString::from_str(&format!("![]({}/x.png)\n", fx.path.parent().unwrap().display())));
        let b = fx.path.parent().unwrap().join("b");
        std::fs::create_dir_all(&b).unwrap();
        let finished = std::rc::Rc::new(std::cell::Cell::new(false));
        let f = finished.clone();
        let typed = e.clone();
        let done = block2::RcBlock::new(move |err: *mut NSError| {
            assert!(err.is_null(), "save succeeds");
            // SAFETY: an NSString is a valid insertText: argument.
            unsafe { typed.insertText_replacementRange(ns_string!("z"), NSRange::new(NSNotFound as usize, 0)) };
            f.set(true);
        });
        let url = NSURL::fileURLWithPath(&NSString::from_str(b.join("new.md").to_str().unwrap()));
        fx.doc.saveToURL_ofType_forSaveOperation_completionHandler(&url, ns_string!(MD), objc2_app_kit::NSSaveOperationType::SaveAsOperation, &done);
        wait_until(|| finished.get());
        ui::pump(0.1);
        assert!(fx.text().ends_with('z'));
        assert!(fx.doc.isDocumentEdited(), "unsaved typing never reads as saved");
    });
}

fn set_mode(p: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Save As into a folder that can't take the file: the rebase and its copies are undone.
fn failed_save_reverts_rebase() {
    on_disk("rebase-fail", "a
", |fx| {
        let dir = fx.path.parent().unwrap();
        std::fs::create_dir_all(dir.join("doc.assets")).unwrap();
        std::fs::write(dir.join("doc.assets/p.png"), b"mine").unwrap();
        fx.doc.editor().unwrap().setString(ns_string!("![](doc.assets/p.png)\n"));
        fx.type_text("x");
        let before = fx.text();
        // The assets folder takes the copy; the read-only folder refuses the document.
        let b = dir.join("b");
        std::fs::create_dir_all(b.join("new.assets")).unwrap();
        set_mode(&b, 0o555);
        let ok = try_save_op(&fx.doc, &b.join("new.md"), objc2_app_kit::NSSaveOperationType::SaveAsOperation);
        set_mode(&b, 0o755);
        assert!(!ok, "the save fails");
        assert_eq!(fx.text(), before);
        assert_eq!(std::fs::read_dir(b.join("new.assets")).unwrap().count(), 0, "no stray copies");
        assert!(fx.doc.isDocumentEdited());
        assert_eq!(fx.doc.fileURL().unwrap().path().unwrap().to_string(), fx.path.to_str().unwrap());
    });
}

/// An image that can't be copied keeps pointing at its original; the save goes ahead.
fn failed_copy_keeps_original_link() {
    on_disk("copy-fail", "a
", |fx| {
        let src = fx.path.parent().unwrap().join("doc.assets/p.png");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"mine").unwrap();
        set_mode(&src, 0o000);
        fx.doc.editor().unwrap().setString(ns_string!("![](doc.assets/p.png)\n"));
        let b = fx.path.parent().unwrap().join("b");
        std::fs::create_dir_all(&b).unwrap();
        let ok = try_save_op(&fx.doc, &b.join("new.md"), objc2_app_kit::NSSaveOperationType::SaveAsOperation);
        let status = fx.status();
        set_mode(&src, 0o644);
        assert!(ok, "the save goes ahead");
        assert_eq!(fx.text(), "![](../doc.assets/p.png)\n");
        assert!(!b.join("new.assets/p.png").exists());
        assert!(status.starts_with("Couldn't copy p.png: "), "{status}");
    });
}

fn autosave_never_rebases() {
    on_disk("autosave-no-rebase", "a\n", |fx| {
        fx.type_text("![](/abs/x.png)");
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "![](/abs/x.png)a\n");
    });
}

fn save_to_never_rebases() {
    on_disk("save-to-no-rebase", "a\n", |fx| {
        fx.doc.editor().unwrap().setString(ns_string!("![](/abs/x.png)\n"));
        let copy = fx.path.parent().unwrap().join("copy.md");
        save_op(&fx.doc, &copy, objc2_app_kit::NSSaveOperationType::SaveToOperation);
        assert_eq!(std::fs::read_to_string(copy).unwrap(), "![](/abs/x.png)\n");
        assert_eq!(fx.text(), "![](/abs/x.png)\n");
    });
}

// ---- Modes ----

fn content_of(win: &objc2_app_kit::NSWindow) -> Retained<DocumentView> {
    win.contentView().expect("content").downcast::<DocumentView>().expect("DocumentView")
}

fn file_opens_in_preview_untitled_in_edit() {
    let d = doc_from(b"# hi\n");
    d.makeWindowControllers();
    let v = content_of(&document_window(&d));
    assert_eq!(v.mode(), Mode::Preview);
    assert_eq!(v.toolbar().selected(), Mode::Preview);
    assert!(v.preview_html().expect("rendered").contains("<h1>hi</h1>"));
    d.close();
    let u = PrasastiDocument::new(mtm());
    u.makeWindowControllers();
    let v = content_of(&document_window(&u));
    assert_eq!(v.mode(), Mode::Edit);
    assert_eq!(v.preview_html(), None, "the web view is created on first switch");
    u.close();
}

fn cmd_e_toggles_mode() {
    let (win, e) = ui::window_with("x");
    let v = content_of(&win);
    assert_eq!(v.mode(), Mode::Edit);
    ui::shortcut(&win, "e", CMD);
    assert_eq!(v.mode(), Mode::Preview);
    assert_eq!(v.toolbar().selected(), Mode::Preview);
    assert!(e.enclosingScrollView().unwrap().isHidden());
    ui::shortcut(&win, "e", CMD);
    assert_eq!(v.mode(), Mode::Edit);
    assert_eq!(v.toolbar().selected(), Mode::Edit);
    assert!(!e.enclosingScrollView().unwrap().isHidden());
    assert!(is_first_responder(&win, &e));
    win.close();
}

fn toolbar_click_switches_mode() {
    let (win, _e) = ui::window_with("x");
    let v = content_of(&win);
    let [preview, edit] = v.toolbar().buttons();
    for (b, want) in [(&preview, Mode::Preview), (&edit, Mode::Edit)] {
        // SAFETY: no sender, as a mouse click has none here.
        unsafe { b.performClick(None) };
        assert_eq!(v.mode(), want);
        assert_eq!(v.toolbar().selected(), want);
    }
    win.close();
}

fn toolbar_thumb_tracks_mode() {
    let (win, _e) = ui::window_with("x");
    win.orderFrontRegardless(); // on screen, so the thumb slides
    let v = content_of(&win);
    let [preview, edit] = v.toolbar().buttons();
    for (m, b) in [(Mode::Preview, &preview), (Mode::Edit, &edit), (Mode::Preview, &preview)] {
        v.set_mode(m);
        ui::pump(0.6);
        let (t, f) = (v.toolbar().thumb().frame(), b.frame());
        let dx = (t.origin.x + t.size.width / 2.0) - (f.origin.x + f.size.width / 2.0);
        assert!(dx.abs() <= 1.0, "{m:?}: thumb {t:?} button {f:?}");
    }
    win.close();
}

fn toolbar_accessibility() {
    use objc2_app_kit::NSAccessibility;
    let (win, _e) = ui::window_with("x");
    let v = content_of(&win);
    let [preview, edit] = v.toolbar().buttons();
    assert_eq!(preview.title().to_string(), "Preview");
    assert_eq!(edit.title().to_string(), "Edit");
    for (m, on) in [(Mode::Preview, true), (Mode::Edit, false)] {
        v.set_mode(m);
        assert_eq!(preview.isAccessibilitySelected(), on);
        assert_eq!(edit.isAccessibilitySelected(), !on);
    }
    for b in [&preview, &edit] {
        assert_eq!(b.accessibilityRole().map(|r| r.to_string()), Some(unsafe { objc2_app_kit::NSAccessibilityRadioButtonRole }.to_string()));
    }
    win.close();
}

/// The text of the toolbar's centred title item.
fn toolbar_title(win: &objc2_app_kit::NSWindow) -> String {
    let tb = win.toolbar().expect("toolbar");
    let item = tb.items().iter().find(|i| i.itemIdentifier().to_string() == prasasti::titlebar::TITLE_ITEM).expect("title item");
    item.view().expect("view").downcast::<objc2_app_kit::NSTextField>().expect("label").stringValue().to_string()
}

fn window_has_toolbar_with_title() {
    on_disk("title", "a\n", |fx| {
        let tb = fx.win.toolbar().expect("toolbar");
        assert!(!tb.allowsUserCustomization());
        assert!(!tb.autosavesConfiguration());
        assert_eq!(fx.win.toolbarStyle(), objc2_app_kit::NSWindowToolbarStyle::Unified);
        assert_eq!(fx.win.title().to_string(), fx.doc.displayName().to_string());
        assert_eq!(toolbar_title(&fx.win), fx.win.title().to_string());
        let new = fx.path.with_file_name("moved.md");
        let url = NSURL::fileURLWithPath(&NSString::from_str(new.to_str().unwrap()));
        let done = std::rc::Rc::new(std::cell::Cell::new(false));
        let d = done.clone();
        let block = block2::RcBlock::new(move |_: *mut NSError| d.set(true));
        fx.doc.saveToURL_ofType_forSaveOperation_completionHandler(&url, ns_string!(MD), objc2_app_kit::NSSaveOperationType::SaveAsOperation, &block);
        wait_until(|| done.get());
        wait_until(|| toolbar_title(&fx.win) == fx.win.title().to_string() && fx.win.title().to_string().starts_with("moved"));
    });
}

fn appearance_button_cycles() {
    use objc2_app_kit::{NSAccessibility, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
    let app = NSApplication::sharedApplication(mtm());
    let pinned = app.appearance();
    on_disk("appearance", "a\n", |fx| {
        let b = prasasti::titlebar::appearance_button(&fx.win).expect("appearance button");
        let name = || app.appearance().map(|a| a.name().to_string());
        let stored = || test_defaults().stringForKey(ns_string!("appearance")).map(|s| s.to_string());
        test_defaults().removeObjectForKey(ns_string!("appearance"));
        settings::set_appearance("system");
        for (want, ns) in [("light", Some(unsafe { NSAppearanceNameAqua })), ("dark", Some(unsafe { NSAppearanceNameDarkAqua })), ("system", None)] {
            click(&b);
            assert_eq!(name(), ns.map(|n| n.to_string()), "{want}");
            assert_eq!(stored().as_deref(), Some(want));
            let label = format!("Appearance: {}{}", want[..1].to_uppercase(), &want[1..]);
            assert_eq!(b.toolTip().map(|s| s.to_string()), Some(label.clone()));
            assert_eq!(b.accessibilityLabel().map(|s| s.to_string()), Some(label));
        }
    });
    test_defaults().removeObjectForKey(ns_string!("appearance"));
    app.setAppearance(pinned.as_deref());
}

fn editor_clears_toolbar() {
    on_disk("insets", "a\n", |fx| {
        let v = content_of(&fx.win);
        v.layoutSubtreeIfNeeded();
        let safe = v.safeAreaInsets().top;
        let bar = v.frame().size.height - fx.win.contentLayoutRect().size.height;
        assert!(safe >= bar && bar > 30.0, "safe {safe} bar {bar}");
        let scroll = fx.doc.editor().unwrap().enclosingScrollView().expect("scroll");
        assert!(scroll.contentInsets().top >= bar, "{} < {bar}", scroll.contentInsets().top);
    });
}

fn preview_ignores_typing_and_edit_commands() {
    let (win, e) = ui::window_with("- a");
    e.setSelectedRange(NSRange::new(3, 0));
    ui::type_text(&win, "q");
    ui::pump(0.05);
    let before = e.string().to_string();
    let v = content_of(&win);
    ui::shortcut(&win, "e", CMD);
    assert_eq!(v.mode(), Mode::Preview);
    ui::type_text(&win, "zz");
    assert!(!ui::enabled(&win, "b", CMD), "formatting is disabled in Preview");
    assert!(!ui::enabled(&win, "z", CMD), "undo is disabled in Preview");
    assert!(!ui::enabled(&win, "v", CMD), "paste is disabled in Preview");
    assert!(!ui::enabled(&win, "x", CMD), "cut is disabled in Preview");
    ui::key(&win, "\t", NSEventModifierFlags::empty());
    ui::pump(0.2);
    assert_eq!(e.string().to_string(), before);
    ui::shortcut(&win, "e", CMD);
    ui::type_text(&win, "x");
    assert_eq!(e.string().to_string(), "- aqx");
    win.close();
}

fn cmd_f_in_preview_opens_find_in_edit() {
    let (win, e) = ui::window_with("x");
    let v = content_of(&win);
    v.set_mode(Mode::Preview);
    ui::shortcut(&win, "f", CMD);
    assert_eq!(v.mode(), Mode::Edit);
    assert!(!find_bar(&e).isHidden());
    win.close();
}

fn preview_rerenders_after_reload() {
    on_disk("rerender", "a\n", |fx| {
        fx.type_text("mine ");
        let v = content_of(&fx.win);
        v.set_mode(Mode::Preview);
        later();
        fx.write("theirs\n");
        fx.doc.disk_changed();
        assert_eq!(fx.doc.hold(), Some(HoldReason::ExternalChange));
        fx.doc.resolve_external(true);
        assert_eq!(fx.text(), "theirs\n");
        assert_eq!(fx.doc.hold(), None);
        let start = Instant::now();
        while !v.preview_html().unwrap_or_default().contains("theirs") && start.elapsed().as_secs_f64() < 0.5 {
            ui::pump(0.02);
        }
        assert!(v.preview_html().unwrap_or_default().contains("theirs"), "{:?}", v.preview_html());
    });
}

fn autosave_unaffected_by_mode() {
    on_disk("autosave-mode", "a\n", |fx| {
        fx.type_text("b");
        ui::shortcut(&fx.win, "e", CMD);
        assert_eq!(content_of(&fx.win).mode(), Mode::Preview);
        assert!(fx.autosave(true));
        assert_eq!(fx.disk(), "ba\n");
    });
}

fn view_menu_checks_active_mode() {
    let (win, _e) = ui::window_with("x");
    let v = content_of(&win);
    let view = NSApplication::sharedApplication(mtm()).mainMenu().unwrap().itemWithTitle(ns_string!("View")).unwrap().submenu().unwrap();
    let preview = view.itemWithTitle(ns_string!("Preview")).expect("View > Preview");
    let edit = view.itemWithTitle(ns_string!("Edit")).expect("View > Edit");
    let on = objc2_app_kit::NSControlStateValueOn;
    let state = |item: &objc2_app_kit::NSMenuItem| {
        // SAFETY: `-(BOOL)validateMenuItem:(NSMenuItem *)item`.
        let ok: bool = unsafe { objc2::msg_send![&*v, validateMenuItem: item] };
        assert!(ok);
        item.state() == on
    };
    assert_eq!((state(&preview), state(&edit)), (false, true));
    v.set_mode(Mode::Preview);
    assert_eq!((state(&preview), state(&edit)), (true, false));
    win.close();
}

/// A temp copy of the kitchen-sink fixture folder (the caller removes it).
fn kitchen_sink_copy(name: &str) -> std::path::PathBuf {
    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if src.is_dir() { copy_dir(&src, &dst) } else { std::fs::copy(&src, &dst).unwrap(); }
        }
    }
    let dir = std::env::temp_dir().join(format!("prasasti-ks-{name}-{}", std::process::id()));
    copy_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/kitchen-sink"), &dir);
    dir
}

/// Pumps until `js` evaluates to `want` in the preview; panics after `secs`.
fn wait_for_js(web: &PreviewView, js: &str, want: &str, secs: f64) {
    let start = Instant::now();
    loop {
        let got = web.eval(js, 1.0);
        if got.as_deref() == Some(want) {
            return;
        }
        assert!(start.elapsed().as_secs_f64() < secs, "{js} is {got:?}, not {want:?}");
        ui::pump(0.05);
    }
}

fn kitchen_sink_renders() {
    let dir = kitchen_sink_copy("render");
    let doc = disk_doc(&dir.join("kitchen-sink.md"));
    doc.makeWindowControllers();
    let v = content_of(&document_window(&doc));
    assert_eq!(v.mode(), Mode::Preview);
    let web = v.preview().expect("preview");
    wait_for_js(&web, "String(document.body && document.body.dataset.rendered)", "1", 10.0);
    let js = |s: &str| web.eval(s, 5.0).unwrap_or_default();
    assert_ne!(js("String(document.querySelectorAll('.katex').length)"), "0", "KaTeX rendered");
    assert_eq!(js("String(document.querySelectorAll('.mermaid > svg').length)"), "3", "Pele rendered");
    assert_eq!(js("String(document.querySelectorAll('.katex-error, .render-error').length)"), "2", "one math, one Mermaid error: {}", js("[...document.querySelectorAll('.katex-error, .render-error, .math-display')].map(e => e.className + ': ' + e.textContent.slice(0, 60)).join(' | ')"));
    // Local images, minus the deliberately broken one and the CSP probe's `src=x`.
    let local = "[...document.images].filter(i => i.src.startsWith('prasasti://file') && !/(missing\\.png|\\/x)$/.test(i.src))";
    wait_for_js(&web, &format!("String({local}.every(i => i.complete))"), "true", 5.0);
    assert_eq!(js(&format!("String({local}.length)")), "4");
    assert_eq!(js(&format!("{local}.filter(i => i.naturalWidth === 0).map(i => i.src).join(' ')")), "", "every local image loads");
    assert_eq!(js("typeof window.__xss"), "undefined", "inline script and onerror never run");
    // The served copy, not a system-wide install: only it has this family name.
    wait_for_js(
        &web,
        "String([...document.fonts].some(f => f.family.replace(/[\"']/g, '') === 'Prasasti JetBrains Mono' && f.status === 'loaded'))",
        "true",
        5.0,
    );
    doc.close();
    let _ = std::fs::remove_dir_all(&dir);
}

fn preview_code_blocks_highlighted_numbered_and_copy_clean() {
    let (win, _e) = ui::window_with("```rust\nfn main() {\n    let x = 1;\n}\n```\n\n```\nplain\n  text\n```\n");
    let v = content_of(&win);
    v.set_mode(Mode::Preview);
    let web = v.preview().expect("preview");
    wait_for_js(&web, "String(document.body && document.body.dataset.rendered)", "1", 10.0);
    let js = |s: &str| web.eval(s, 5.0).unwrap_or_default();
    wait_for_js(&web, "String(document.fonts.check('15px \"Prasasti JetBrains Mono\"'))", "true", 5.0);
    assert!(js("getComputedStyle(document.querySelector('pre code')).fontFamily").contains("JetBrains Mono"));
    assert_ne!(js("String(document.querySelectorAll('pre code .hljs-keyword').length)"), "0", "keyword token");
    assert_eq!(js("String(document.querySelectorAll('pre:nth-of-type(1) code .line').length)"), "3");
    assert_eq!(js("String(document.querySelectorAll('pre:nth-of-type(2) code [class^=hljs]').length)"), "0", "untagged block is not highlighted");
    assert_eq!(js("String(document.querySelectorAll('pre:nth-of-type(2) code .line').length)"), "2", "but still numbered");
    assert_eq!(js("String(document.querySelectorAll('.ind').length)"), "2", "one guide per indented line");
    assert_eq!(js("getComputedStyle(document.querySelector('.line'), '::before').content"), "counter(ln)", "numbers are generated content");
    js("(() => { const r = document.createRange(); r.selectNodeContents(document.querySelector('pre code')); const s = getSelection(); s.removeAllRanges(); s.addRange(r); return 1; })()");
    assert_eq!(js("getSelection().toString()"), "fn main() {\n    let x = 1;\n}");
    win.close();
}

fn preview_never_navigates_away() {
    let (win, _e) = ui::window_with(
        "<meta http-equiv=\"refresh\" content=\"0;url=https://example.com\">\n\n<form action=\"https://example.com\"><button>x</button></form>\n",
    );
    let v = content_of(&win);
    v.set_mode(Mode::Preview);
    let web = v.preview().expect("preview");
    wait_for_js(&web, "String(document.body && document.body.dataset.rendered)", "1", 10.0);
    web.eval("document.querySelector('form').submit(); 'ok'", 1.0);
    ui::pump(1.0);
    let href = web.eval("location.href", 1.0).unwrap_or_default();
    assert!(href.starts_with("prasasti://file/"), "{href}");
    win.close();
}

fn image_beside_doc_in_odd_folder_loads() {
    let dir = std::env::temp_dir().join(format!("prasasti a #%ü-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/kitchen-sink/assets/logo.png");
    std::fs::copy(fixture, dir.join("logo.png")).unwrap();
    std::fs::write(dir.join("doc.md"), "![logo](logo.png)\n").unwrap();
    let doc = disk_doc(&dir.join("doc.md"));
    doc.makeWindowControllers();
    let web = content_of(&document_window(&doc)).preview().expect("preview");
    wait_for_js(&web, "String(document.images.length === 1 && document.images[0].complete)", "true", 10.0);
    let width = web.eval("String(document.images[0].naturalWidth)", 1.0).unwrap_or_default();
    let src = web.eval("document.images[0].src", 1.0).unwrap_or_default();
    doc.close();
    let _ = std::fs::remove_dir_all(&dir);
    assert_ne!(width, "0", "{src}");
}

fn kitchen_sink_unchanged_after_open_close() {
    let dir = kitchen_sink_copy("roundtrip");
    let path = dir.join("kitchen-sink.md");
    let before = std::fs::read(&path).unwrap();
    let doc = disk_doc(&path);
    doc.makeWindowControllers();
    let win = document_window(&doc);
    ui::shortcut(&win, "e", CMD);
    assert_eq!(content_of(&win).mode(), Mode::Edit);
    ui::shortcut(&win, "e", CMD);
    assert_eq!(content_of(&win).mode(), Mode::Preview);
    ui::pump(0.3);
    assert!(!doc.isDocumentEdited());
    doc.close();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

fn main() {
    register_bundled_fonts();
    let app = NSApplication::sharedApplication(mtm());
    // Pin the light (Aqua) appearance before any window: colour assertions use
    // the light palette and must not depend on the system's Dark mode setting.
    app.setAppearance(objc2_app_kit::NSAppearance::appearanceNamed(unsafe { objc2_app_kit::NSAppearanceNameAqua }).as_deref());
    // Never the user's real defaults domain.
    settings::set_defaults(Some(test_defaults()));
    fresh_defaults();
    // Window frame autosave writes to the standard domain; put the user's frame back at the end.
    let std = NSUserDefaults::standardUserDefaults();
    let saved_frame = std.stringForKey(&NSString::from_str(FRAME_KEY));
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
        ("copy_as_html_writes_both_types", copy_as_html_writes_both_types),
        ("paste_and_match_style_normalizes_crlf", paste_and_match_style_normalizes_crlf),
        ("app_menu_has_standard_items", app_menu_has_standard_items),
        ("print_operation_builds", print_operation_builds),
        ("print_uses_editor_font_top_aligned", print_uses_editor_font_top_aligned),
        ("print_remote_image_times_out", print_remote_image_times_out),
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
        ("default_font_is_jetbrains_mono_15", default_font_is_jetbrains_mono_15),
        ("zoom_in_changes_point_size", zoom_in_changes_point_size),
        ("zoom_flashes_text_size", zoom_flashes_text_size),
        ("status_flash_clears_after_2s", status_flash_clears_after_2s),
        ("zoom_applies_to_every_window", zoom_applies_to_every_window),
        ("change_font_stores_family", change_font_stores_family),
        ("own_write_ignored", own_write_ignored),
        ("clean_doc_reloads_silently", clean_doc_reloads_silently),
        ("edited_doc_holds", edited_doc_holds),
        ("autosave_skipped_while_held", autosave_skipped_while_held),
        ("autosave_runs_when_not_held", autosave_runs_when_not_held),
        ("keep_mine_then_save_overwrites", keep_mine_then_save_overwrites),
        ("reload_discards", reload_discards),
        ("deleted_file_holds", deleted_file_holds),
        ("removed_file_returns_changed", removed_file_returns_changed),
        ("crlf_hold_blocks_autosave_until_save", crlf_hold_blocks_autosave_until_save),
        ("paused_status_outlives_transient_status", paused_status_outlives_transient_status),
        ("rename_follows_file", rename_follows_file),
        ("close_while_held_keeps_document", close_while_held_keeps_document),
        ("sheet_keep_mine_button", sheet_keep_mine_button),
        ("deleted_sheet_save_writes_back", deleted_sheet_save_writes_back),
        ("recovered_draft_keeps_hold", recovered_draft_keeps_hold),
        ("untitled_draft_has_no_hold", untitled_draft_has_no_hold),
        ("init_with_type_has_ivars", init_with_type_has_ivars),
        ("init_with_contents_has_ivars", init_with_contents_has_ivars),
        ("drafts_rotate_keeping_two", drafts_rotate_keeping_two),
        ("hold_change_replaces_sheet", hold_change_replaces_sheet),
        ("presenter_reloads_clean_doc", presenter_reloads_clean_doc),
        ("document_class_registered_by_name", document_class_registered_by_name),
        ("info_plist_classes_resolve", info_plist_classes_resolve),
        ("typing_after_autosave_counts", typing_after_autosave_counts),
        ("typing_during_autosave_counts", typing_during_autosave_counts),
        ("autosave_checks_disk_first", autosave_checks_disk_first),
        ("autosave_checks_missing_file", autosave_checks_missing_file),
        ("save_checks_disk_first", save_checks_disk_first),
        ("rechecks_disk_after_save", rechecks_disk_after_save),
        ("close_dialog_save_checks_disk", close_dialog_save_checks_disk),
        ("save_op_cancels_when_disk_changed", save_op_cancels_when_disk_changed),
        ("recheck_after_keep_mine_stays_quiet", recheck_after_keep_mine_stays_quiet),
        ("held_controller_doc_writes_draft", held_controller_doc_writes_draft),
        ("controller_doc_elsewhere_op_writes_draft", controller_doc_elsewhere_op_writes_draft),
        ("draft_name_with_slash_is_written", draft_name_with_slash_is_written),
        ("failed_save_clears_snapshot", failed_save_clears_snapshot),
        ("duplicate_clears_snapshot", duplicate_clears_snapshot),
        ("recheck_skips_closed_document", recheck_skips_closed_document),
        ("paste_undoes_alone", paste_undoes_alone),
        ("glass_and_webkit_available", glass_and_webkit_available),
        ("kitchen_sink_is_well_formed", kitchen_sink_is_well_formed),
        ("hold_shows_status_pill", hold_shows_status_pill),
        ("window_has_no_visible_titlebar", window_has_no_visible_titlebar),
        ("column_width_fixed", column_width_fixed),
        ("window_min_width_follows_column", window_min_width_follows_column),
        ("restored_narrow_window_grows", restored_narrow_window_grows),
        ("kitchen_sink_lines_fit_column", kitchen_sink_lines_fit_column),
        ("kitchen_sink_renders", kitchen_sink_renders),
        ("preview_code_blocks_highlighted_numbered_and_copy_clean", preview_code_blocks_highlighted_numbered_and_copy_clean),
        ("preview_never_navigates_away", preview_never_navigates_away),
        ("kitchen_sink_unchanged_after_open_close", kitchen_sink_unchanged_after_open_close),
        ("image_beside_doc_in_odd_folder_loads", image_beside_doc_in_odd_folder_loads),
        ("frame_restored_exactly", frame_restored_exactly),
        ("second_window_cascades", second_window_cascades),
        ("last_closed_window_frame_is_stored", last_closed_window_frame_is_stored),
        ("resize_in_place_is_stored_without_closing", resize_in_place_is_stored_without_closing),
        ("no_window_tabbing", no_window_tabbing),
        ("file_opens_in_preview_untitled_in_edit", file_opens_in_preview_untitled_in_edit),
        ("cmd_e_toggles_mode", cmd_e_toggles_mode),
        ("toolbar_click_switches_mode", toolbar_click_switches_mode),
        ("toolbar_thumb_tracks_mode", toolbar_thumb_tracks_mode),
        ("toolbar_accessibility", toolbar_accessibility),
        ("window_has_toolbar_with_title", window_has_toolbar_with_title),
        ("appearance_button_cycles", appearance_button_cycles),
        ("editor_clears_toolbar", editor_clears_toolbar),
        ("preview_ignores_typing_and_edit_commands", preview_ignores_typing_and_edit_commands),
        ("cmd_f_in_preview_opens_find_in_edit", cmd_f_in_preview_opens_find_in_edit),
        ("preview_rerenders_after_reload", preview_rerenders_after_reload),
        ("autosave_unaffected_by_mode", autosave_unaffected_by_mode),
        ("view_menu_checks_active_mode", view_menu_checks_active_mode),
        ("first_save_rebases_images", first_save_rebases_images),
        ("save_as_moves_relative_and_carries_assets", save_as_moves_relative_and_carries_assets),
        ("carry_never_overwrites", carry_never_overwrites),
        ("autosave_never_rebases", autosave_never_rebases),
        ("failed_save_reverts_rebase", failed_save_reverts_rebase),
        ("typing_after_rebasing_save_stays_edited", typing_after_rebasing_save_stays_edited),
        ("failed_copy_keeps_original_link", failed_copy_keeps_original_link),
        ("save_to_never_rebases", save_to_never_rebases),
    ];
    let mut failed = 0;
    for (name, f) in tests {
        let ok = autoreleasepool(|_| std::panic::catch_unwind(f).is_ok());
        println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
        failed += usize::from(!ok);
    }
    println!("\n{} passed; {failed} failed", tests.len() - failed);
    match saved_frame {
        Some(v) => unsafe { std.setObject_forKey(Some(&v), &NSString::from_str(FRAME_KEY)) },
        None => std.removeObjectForKey(&NSString::from_str(FRAME_KEY)),
    }
    if failed > 0 {
        std::process::exit(1);
    }
}
