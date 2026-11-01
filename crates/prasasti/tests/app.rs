//! Main-thread AppKit tests (harness = false: `main` runs on the main thread).

use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSApplication, NSDocument, NSTextInputClient};
use objc2_foundation::{MainThreadMarker, NSData, NSError, NSNotFound, NSRange, ns_string};
use prasasti::document::{HoldReason, PrasastiDocument};

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

fn main() {
    let _app = NSApplication::sharedApplication(mtm());
    let tests: &[(&str, fn())] = &[
        ("opens_utf8", opens_utf8),
        ("refuses_non_utf8", refuses_non_utf8),
        ("crlf_holds_autosave", crlf_holds_autosave),
        ("writes_utf8_lf", writes_utf8_lf),
        ("window_shows_text_and_typing_edits", window_shows_text_and_typing_edits),
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
