use std::cell::{Cell, RefCell};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::{DynBlock, RcBlock};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSBackingStoreType, NSDocument,
    NSDocumentChangeType, NSLayoutAttribute, NSModalResponse, NSSaveOperationType, NSStackView,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowController, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSCocoaErrorDomain, NSCopying, NSData, NSDate, NSDictionary, NSError,
    NSFileReadInapplicableStringEncodingError, NSFileWriteUnknownError, NSLocalizedDescriptionKey,
    NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString, NSURL,
    NSURLContentModificationDateKey, ns_string,
};
use prasasti_core::file::{decode, normalize_newlines, saves_unchanged};

use crate::editor::{EditorView, editor_in_scroll};
use crate::find_bar::FindBar;
use crate::footer::Footer;
use crate::util::guarded;

/// Whether external-change and deletion sheets are shown (off in tests).
pub static SHEETS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Why autosave must not write the file right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// The file on disk has CR line endings; saving would rewrite them.
    LineEndings,
    ExternalChange,
    Deleted,
}

pub struct Ivars {
    autosave_hold: Cell<Option<HoldReason>>,
    /// Bytes last read from or written to disk, for external-change checks.
    last_known: RefCell<Option<Vec<u8>>>,
    /// Bytes `dataOfType` produced for the save in progress; `last_known` once it succeeds.
    pending: RefCell<Option<Vec<u8>>>,
    /// The text until a window exists; afterwards the editor's storage is the truth.
    text: RefCell<Retained<NSString>>,
    editor: RefCell<Option<Retained<EditorView>>>,
}

define_class!(
    // SAFETY: NSDocument is designed for subclassing; no Drop.
    #[unsafe(super(NSDocument))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct PrasastiDocument;

    unsafe impl NSObjectProtocol for PrasastiDocument {}

    impl PrasastiDocument {
        #[unsafe(method(autosavesInPlace))]
        fn autosaves_in_place() -> bool {
            true
        }

        #[unsafe(method(makeWindowControllers))]
        fn make_window_controllers(&self) {
            let mtm = self.mtm();
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(720.0, 640.0));
            // SAFETY: released-when-closed is turned off right below; the
            // window controller owns the window.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Miniaturizable
                        | NSWindowStyleMask::Resizable,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };

            let (content, editor) = window_content(frame);
            editor.setString(&self.ivars().text.borrow());
            window.setContentView(Some(&content));
            window.makeFirstResponder(Some(&editor));

            let controller = NSWindowController::initWithWindow(
                NSWindowController::alloc(mtm),
                Some(&window),
            );
            window.center();
            controller.setWindowFrameAutosaveName(ns_string!("PrasastiMain"));
            self.addWindowController(&controller);
            *self.ivars().editor.borrow_mut() = Some(editor);
        }

        #[unsafe(method(readFromData:ofType:error:))]
        fn read_from_data(
            &self,
            data: &NSData,
            _type_name: &NSString,
            error: *mut *mut NSError,
        ) -> Bool {
            let raw = data.to_vec();
            let decoded = guarded(|| {
                decode(&raw).map(|s| {
                    if saves_unchanged(&raw) { (s, None) } else { (normalize_newlines(&s), Some(HoldReason::LineEndings)) }
                })
            });
            let Some(Ok((text, hold))) = decoded else {
                set_error(error, not_utf8_error());
                return Bool::NO;
            };
            let ivars = self.ivars();
            ivars.autosave_hold.set(hold);
            *ivars.last_known.borrow_mut() = Some(raw);
            *ivars.text.borrow_mut() = NSString::from_str(&text);
            if let Some(editor) = &*ivars.editor.borrow() {
                editor.setString(&ivars.text.borrow());
            }
            Bool::YES
        }

        #[unsafe(method_id(dataOfType:error:))]
        fn data_of_type(&self, _type_name: &NSString, _error: *mut *mut NSError) -> Retained<NSData> {
            let text = self.text().to_string();
            // Always UTF-8 with LF, even if CRs were pasted in.
            let bytes = guarded(|| normalize_newlines(&text).into_bytes()).unwrap_or(text.into_bytes());
            let data = NSData::with_bytes(&bytes);
            *self.ivars().pending.borrow_mut() = Some(bytes);
            data
        }

        #[unsafe(method(autosaveWithImplicitCancellability:completionHandler:))]
        fn autosave(&self, cancellable: bool, handler: &DynBlock<dyn Fn(*mut NSError)>) {
            match self.hold() {
                None => unsafe {
                    msg_send![super(self), autosaveWithImplicitCancellability: cancellable, completionHandler: handler]
                },
                // A timed autosave is simply skipped.
                Some(_) if cancellable => handler.call((ptr::null_mut(),)),
                // A forced one (close, quit) must fail: reporting success would
                // let AppKit close the window and drop the text typed since the hold.
                Some(reason) => call_with_error(handler, &held_error(reason)),
            }
        }

        #[unsafe(method(saveToURL:ofType:forSaveOperation:completionHandler:))]
        fn save_to_url(
            &self,
            url: &NSURL,
            type_name: &NSString,
            op: NSSaveOperationType,
            handler: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            // Backstop for any in-place autosave that does not go through the method above.
            if op == NSSaveOperationType::AutosaveInPlaceOperation {
                if let Some(reason) = self.hold() {
                    return call_with_error(handler, &held_error(reason));
                }
            }
            let this = self.retain();
            let handler = handler.copy();
            let done = RcBlock::new(move |err: *mut NSError| {
                if err.is_null() {
                    guarded(|| this.saved(op));
                }
                handler.call((err,));
            });
            unsafe {
                msg_send![super(self), saveToURL: url, ofType: type_name, forSaveOperation: op, completionHandler: &*done]
            }
        }

        // The NSFilePresenter callbacks arrive on the presenter queue, never
        // main: each hops to main before touching AppKit or the ivars.

        // Not super: NSDocument's own handling would revert or re-check the
        // file in parallel with `disk_changed`, losing the caret or racing it.
        #[unsafe(method(presentedItemDidChange))]
        fn presented_item_did_change(&self) {
            guarded(|| self.on_main(sel!(prasastiDiskChanged)));
        }

        #[unsafe(method(presentedItemDidMoveToURL:))]
        fn presented_item_did_move(&self, url: &NSURL) {
            let trashed = guarded(|| url.path().is_some_and(|p| p.to_string().contains("/.Trash"))).unwrap_or(false);
            if trashed {
                // Moved to the Trash counts as deleted; fileURL stays put, so Save
                // writes the text back where it was.
                self.on_main(sel!(prasastiDeleted));
            } else {
                // super follows the rename (updates fileURL on main).
                unsafe { msg_send![super(self), presentedItemDidMoveToURL: url] }
            }
        }

        // Not super: NSDocument would act on the deletion itself; we keep the
        // text and ask. The handler is called exactly once, right here, so the
        // coordinator never waits on main.
        #[unsafe(method(accommodatePresentedItemDeletionWithCompletionHandler:))]
        fn accommodate_deletion(&self, handler: &DynBlock<dyn Fn(*mut NSError)>) {
            guarded(|| self.on_main(sel!(prasastiDeleted)));
            handler.call((ptr::null_mut(),));
        }

        #[unsafe(method(prasastiDiskChanged))]
        fn disk_changed_on_main(&self) {
            guarded(|| self.disk_changed());
        }

        #[unsafe(method(prasastiDeleted))]
        fn deleted_on_main(&self) {
            guarded(|| self.deleted());
        }
    }
);

impl PrasastiDocument {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            autosave_hold: Cell::new(None),
            last_known: RefCell::new(None),
            pending: RefCell::new(None),
            text: RefCell::new(NSString::new()),
            editor: RefCell::new(None),
        });
        // SAFETY: NSDocument's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// The current text: the editor's when a window exists, else the loaded text.
    pub fn text(&self) -> Retained<NSString> {
        match &*self.ivars().editor.borrow() {
            // A copy: the editor's string is its live, mutable backing store.
            Some(editor) => editor.string().copy(),
            None => self.ivars().text.borrow().clone(),
        }
    }

    pub fn hold(&self) -> Option<HoldReason> {
        self.ivars().autosave_hold.get()
    }

    /// Compares the file with what we last read or wrote: silently reloads a
    /// clean document, holds autosave and asks for an edited one.
    pub fn disk_changed(&self) {
        let Some(path) = self.path() else { return };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return self.deleted(),
            Err(e) => return eprintln!("prasasti: can't read {path}: {e}"),
        };
        if self.ivars().last_known.borrow().as_deref() == Some(&bytes[..]) {
            return; // our own write
        }
        if !self.isDocumentEdited() && self.reload() {
            return;
        }
        self.ivars().autosave_hold.set(Some(HoldReason::ExternalChange));
        let this = self.retain();
        self.sheet(
            "The file changed on disk.",
            "Reload discards your changes. Keep Mine keeps them; auto-save stays paused until you save.",
            ["Reload", "Keep Mine"],
            // Any other way of dismissing keeps the local text.
            move |r| this.resolve_external(r == NSAlertFirstButtonReturn),
        );
    }

    /// The answer to the external-change sheet: Reload (discard mine) or Keep Mine.
    pub fn resolve_external(&self, reload: bool) {
        if reload {
            // On failure the hold stays and the local text is untouched.
            self.reload();
            return;
        }
        *self.ivars().last_known.borrow_mut() = None;
        // The disk no longer matches, even if undo returns to the opened text.
        self.updateChangeCount(NSDocumentChangeType::ChangeDone);
        // NSDocument compares this with the file's date when saving; without it
        // Save asks "changed by another application" instead of overwriting.
        if let Some(date) = self.path().and_then(|p| disk_date(&p)) {
            self.setFileModificationDate(Some(&date));
        }
    }

    /// The file is gone: hold autosave and offer Save (write it back) or Close.
    fn deleted(&self) {
        if self.hold() == Some(HoldReason::Deleted) {
            return;
        }
        self.ivars().autosave_hold.set(Some(HoldReason::Deleted));
        let this = self.retain();
        self.sheet(
            "The file was deleted or moved to the Trash.",
            "Save writes your text back to disk. Close discards it.",
            ["Save", "Close"],
            move |r| {
                if r == NSAlertFirstButtonReturn {
                    this.save_back();
                } else if r == NSAlertSecondButtonReturn {
                    this.close();
                }
            },
        );
    }

    /// Saves to the (deleted) file's own URL. `saveDocument:` would instead ask
    /// for a new location, as AppKit thinks it lost track of the file.
    fn save_back(&self) {
        let (Some(url), Some(kind)) = (self.fileURL(), self.fileType()) else { return };
        let this = self.retain();
        let done = RcBlock::new(move |err: *mut NSError| {
            // SAFETY: the handler's error is nil or a valid NSError.
            if let Some(err) = unsafe { err.as_ref() } {
                // The hold stays and the text is untouched; tell the user.
                if let Some(win) = this.windowControllers().firstObject().and_then(|c| c.window()) {
                    NSAlert::alertWithError(err, this.mtm()).beginSheetModalForWindow_completionHandler(&win, None);
                }
            }
        });
        self.saveToURL_ofType_forSaveOperation_completionHandler(&url, &kind, NSSaveOperationType::SaveOperation, &done);
    }

    /// Reverts to the file, keeping the selection (clamped). False if it can't be read.
    fn reload(&self) -> bool {
        let (Some(url), Some(kind)) = (self.fileURL(), self.fileType()) else { return false };
        let selection = self.editor().map(|e| e.selectedRange());
        if let Err(e) = self.revertToContentsOfURL_ofType_error(&url, &kind) {
            eprintln!("prasasti: reload failed: {}", e.localizedDescription());
            return false;
        }
        if let (Some(editor), Some(r)) = (self.editor(), selection) {
            let len = editor.string().length();
            let start = r.location.min(len);
            let end = (r.location + r.length).min(len);
            editor.setSelectedRange(NSRange::new(start, end - start));
        }
        true
    }

    /// After a successful write: an explicit save releases the hold; every
    /// write of the file itself becomes the new last-known contents.
    fn saved(&self, op: NSSaveOperationType) {
        use NSSaveOperationType as Op;
        let pending = self.ivars().pending.take();
        if matches!(op, Op::SaveOperation | Op::SaveAsOperation) {
            self.ivars().autosave_hold.set(None);
        }
        let writes_file = matches!(op, Op::SaveOperation | Op::SaveAsOperation | Op::AutosaveInPlaceOperation | Op::AutosaveAsOperation);
        if writes_file && pending.is_some() {
            *self.ivars().last_known.borrow_mut() = pending;
        }
    }

    fn path(&self) -> Option<String> {
        Some(self.fileURL()?.path()?.to_string())
    }

    fn sheet(&self, message: &str, info: &str, buttons: [&str; 2], on: impl Fn(NSModalResponse) + 'static) {
        if !SHEETS_ENABLED.load(Ordering::SeqCst) {
            return;
        }
        // Our own window only: a closed document must never borrow another's.
        let Some(win) = self.windowControllers().firstObject().and_then(|c| c.window()) else { return };
        if win.attachedSheet().is_some() {
            return; // the open sheet already covers it
        }
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(info));
        for b in buttons {
            alert.addButtonWithTitle(&NSString::from_str(b));
        }
        let done = RcBlock::new(move |r: NSModalResponse| {
            guarded(|| on(r));
        });
        alert.beginSheetModalForWindow_completionHandler(&win, Some(&done));
    }

    /// Runs `sel` (a no-argument method of ours) on the main queue; safe from any thread.
    fn on_main(&self, sel: Sel) {
        // SAFETY: `sel` names a `-(void)x` method of this class; the object is
        // retained until it runs.
        unsafe {
            let _: () = msg_send![self, performSelectorOnMainThread: sel, withObject: None::<&AnyObject>, waitUntilDone: false];
        }
    }

    pub fn editor(&self) -> Option<Retained<EditorView>> {
        self.ivars().editor.borrow().clone()
    }
}

/// The window's content, top to bottom: the (hidden) find bar, the editor, the footer.
pub fn window_content(frame: NSRect) -> (Retained<NSStackView>, Retained<EditorView>) {
    let mtm = MainThreadMarker::new().expect("windows are built on the main thread");
    let (scroll, editor) = editor_in_scroll(frame);
    let bar = FindBar::new(mtm, &editor);
    let footer = Footer::new(mtm, &editor);
    let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(&[&*bar as &NSView, &scroll, &footer]), mtm);
    stack.setFrame(frame);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Width);
    stack.setSpacing(0.0);
    (stack, editor)
}

/// The file's modification date as NSDocument reads it, uncached.
fn disk_date(path: &str) -> Option<Retained<NSDate>> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let key = unsafe { NSURLContentModificationDateKey };
    let values = url.resourceValuesForKeys_error(&NSArray::from_slice(&[key])).ok()?;
    values.objectForKey(key)?.downcast().ok()
}

fn held_error(reason: HoldReason) -> Retained<NSError> {
    let why = match reason {
        HoldReason::LineEndings => "saving would change the file's line endings",
        HoldReason::ExternalChange => "the file changed on disk",
        HoldReason::Deleted => "the file was deleted",
    };
    let msg = format!("Auto-save is paused because {why}. Choose File > Save to keep your changes.");
    let info = NSDictionary::<NSString, AnyObject>::from_slices(
        &[unsafe { NSLocalizedDescriptionKey }],
        &[&*NSString::from_str(&msg) as &AnyObject],
    );
    unsafe { NSError::errorWithDomain_code_userInfo(NSCocoaErrorDomain, NSFileWriteUnknownError, Some(&info)) }
}

fn call_with_error(handler: &DynBlock<dyn Fn(*mut NSError)>, err: &NSError) {
    handler.call((err as *const NSError as *mut NSError,));
}

fn not_utf8_error() -> Retained<NSError> {
    let info = NSDictionary::<NSString, AnyObject>::from_slices(
        &[unsafe { NSLocalizedDescriptionKey }],
        &[&**ns_string!("The file isn't UTF-8 text.")],
    );
    unsafe {
        NSError::errorWithDomain_code_userInfo(
            NSCocoaErrorDomain,
            NSFileReadInapplicableStringEncodingError,
            Some(&info),
        )
    }
}

fn set_error(out: *mut *mut NSError, err: Retained<NSError>) {
    if !out.is_null() {
        // SAFETY: Cocoa's NSError** convention; the caller owns an autorelease pool.
        unsafe { *out = Retained::autorelease_ptr(err) };
    }
}
