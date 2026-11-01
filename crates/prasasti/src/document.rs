use std::cell::{Cell, RefCell};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::{DynBlock, RcBlock};

use objc2::rc::{Allocated, Retained};
use objc2::AnyThread;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAttributedStringAppKitDocumentFormats, NSColor, NSForegroundColorAttributeName, NSLayoutManager, NSPrintInfo, NSPrintOperation, NSTextContainer, NSTextStorage, NSTextView, NSAlertSecondButtonReturn, NSBackingStoreType, NSDocument,
    NSDocumentChangeType, NSLayoutAttribute, NSModalResponse, NSSaveOperationType, NSStackView,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowController, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSCocoaErrorDomain, NSCopying, NSData, NSDate, NSDictionary, NSError,
    NSFeatureUnsupportedError, NSFileReadInapplicableStringEncodingError, NSFileWriteUnknownError, NSLocalizedDescriptionKey, NSUserCancelledError,
    NSFileManager, NSObjectProtocol, NSPoint, NSProcessInfo, NSRange, NSRect, NSSearchPathDirectory,
    NSSearchPathDomainMask, NSSize, NSString, NSURL, NSURLContentModificationDateKey, ns_string,
};
use prasasti_core::file::{decode, normalize_newlines, saves_unchanged};

use crate::editor::{EditorView, editor_in_scroll};
use crate::find_bar::FindBar;
use crate::footer::Footer;
use crate::util::guarded;

/// Whether external-change and deletion sheets are shown (off in tests).
pub static SHEETS_ENABLED: AtomicBool = AtomicBool::new(true);

thread_local! {
    /// Tests only: runs once right after the next save snapshot is taken.
    pub static AFTER_SNAPSHOT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

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
    /// The draft before the current one: kept until the next rotation succeeds,
    /// so restorable state never names a file already deleted.
    previous_draft: RefCell<Option<String>>,
    /// Our open sheet (for which hold) and a counter that retires stale ones.
    sheet: RefCell<Option<(HoldReason, Retained<NSWindow>)>>,
    sheet_gen: Cell<u64>,
}

impl Ivars {
    fn new() -> Self {
        Self {
            autosave_hold: Cell::new(None),
            last_known: RefCell::new(None),
            pending: RefCell::new(None),
            text: RefCell::new(NSString::new()),
            editor: RefCell::new(None),
            previous_draft: RefCell::new(None),
            sheet: RefCell::new(None),
            sheet_gen: Cell::new(0),
        }
    }
}

define_class!(
    // SAFETY: NSDocument is designed for subclassing; no Drop.
    #[unsafe(super(NSDocument))]
    // Info.plist names this class (NSDocumentClass); objc2 would otherwise
    // register a generated, version-suffixed name.
    #[name = "PrasastiDocument"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct PrasastiDocument;

    unsafe impl NSObjectProtocol for PrasastiDocument {}

    impl PrasastiDocument {
        // Every NSDocument initializer (the document controller's included)
        // runs `init` first, so the ivars are always set here.
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(Ivars::new());
            unsafe { msg_send![super(this), init] }
        }

        // AppKit reports reopening a recovery draft (after a crash: contents
        // read from a file other than fileURL) as ReadOtherContents.
        #[unsafe(method(updateChangeCount:))]
        fn update_change_count(&self, change: NSDocumentChangeType) {
            unsafe { msg_send![super(self), updateChangeCount: change] }
            if change == NSDocumentChangeType::ChangeReadOtherContents {
                guarded(|| self.recovered_from_draft());
            }
        }

        #[unsafe(method(saveDocument:))]
        fn save_document(&self, sender: Option<&AnyObject>) {
            // A change the presenter missed gets our sheet, not AppKit's
            // "changed by another application" alert; nothing is written.
            // While a write is in flight the disk is mid-change; the post-save re-check covers it.
            let writing = self.ivars().pending.borrow().is_some();
            if !writing && guarded(|| self.check_disk()).unwrap_or(false) {
                return;
            }
            if self.hold() == Some(HoldReason::Deleted) {
                // AppKit would ask for a new location; the file belongs where it was.
                guarded(|| self.save_back());
            } else {
                unsafe { msg_send![super(self), saveDocument: sender] }
            }
        }

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
            self.set_hold(self.hold());
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
            self.set_hold(hold);
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
            if let Some(f) = AFTER_SNAPSHOT.with(|h| h.borrow_mut().take()) {
                f();
            }
            data
        }

        /// Print renders the Markdown as HTML into an offscreen view as wide as the page's text area.
        #[unsafe(method_id(printOperationWithSettings:error:))]
        fn print_operation(
            &self,
            settings: &NSDictionary<NSString, AnyObject>,
            error: *mut *mut NSError,
        ) -> Option<Retained<NSPrintOperation>> {
            let op = guarded(|| self.build_print_operation(settings)).flatten();
            if op.is_none() {
                set_error(error, print_error());
            }
            op
        }

        #[unsafe(method(autosaveWithImplicitCancellability:completionHandler:))]
        fn autosave(&self, cancellable: bool, handler: &DynBlock<dyn Fn(*mut NSError)>) {
            let handler = handler.copy();
            let inner = handler.clone();
            // The handler must run exactly once; a panic before handing it on reports failure.
            if guarded(|| self.autosave_held_aware(cancellable, inner)).is_none() {
                call_with_error(&handler, &internal_error());
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
            let handler = handler.copy();
            let inner = handler.clone();
            if guarded(|| self.save_held_aware(url, type_name, op, inner)).is_none() {
                call_with_error(&handler, &internal_error());
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
            let trashed = guarded(|| in_trash(url)).unwrap_or(false);
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

        // The post-save re-check: quiet while held (Keep Mine already answered).
        #[unsafe(method(prasastiRecheckDisk))]
        fn recheck_disk_on_main(&self) {
            guarded(|| self.check_disk());
        }

        #[unsafe(method(prasastiDeleted))]
        fn deleted_on_main(&self) {
            guarded(|| self.deleted());
        }
    }
);

impl PrasastiDocument {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: our `init` override sets the ivars.
        unsafe { msg_send![Self::alloc(mtm), init] }
    }

    fn autosave_held_aware(&self, cancellable: bool, handler: RcBlock<dyn Fn(*mut NSError)>) {
        self.check_disk();
        match self.hold() {
            None => unsafe {
                msg_send![super(self), autosaveWithImplicitCancellability: cancellable, completionHandler: &*handler]
            },
            // The file itself is never written while held; a recovery draft
            // is, so a crash or kill can't lose the text.
            Some(reason) => self.write_draft(move |draft_err| {
                if cancellable {
                    handler.call((draft_err,));
                } else {
                    // A forced one (close, quit) must fail even after the
                    // draft: success would let AppKit close the window.
                    call_with_error(&handler, &held_error(reason));
                }
            }),
        }
    }

    fn save_held_aware(&self, url: &NSURL, kind: &NSString, op: NSSaveOperationType, handler: RcBlock<dyn Fn(*mut NSError)>) {
        // NSDocument takes its change-count token now; typing that coalesced
        // into the open undo group would never count as a change (as TextEdit does).
        if let Some(editor) = self.editor() {
            editor.breakUndoCoalescing();
        }
        // Any other Save (the close/quit dialog's goes through
        // saveDocumentWithDelegate:): our sheet instead of AppKit's alert.
        if op == NSSaveOperationType::SaveOperation && self.check_disk() {
            return call_with_error(&handler, &cancelled_error());
        }
        // Backstop for any in-place autosave that does not go through `autosave`.
        if op == NSSaveOperationType::AutosaveInPlaceOperation {
            if let Some(reason) = self.hold() {
                return call_with_error(&handler, &held_error(reason));
            }
        }
        let this = self.retain();
        let done = RcBlock::new(move |err: *mut NSError| {
            if err.is_null() {
                guarded(|| this.saved(op));
            }
            handler.call((err,));
        });
        unsafe { msg_send![super(self), saveToURL: url, ofType: kind, forSaveOperation: op, completionHandler: &*done] }
    }

    /// Reopened from a recovery draft: the draft was written because autosave
    /// was held, so hold it again, judged against the original file.
    fn recovered_from_draft(&self) {
        // Untitled restore and Duplicate also read other contents: no file, no hold.
        let Some(path) = self.path() else { return };
        let (hold, known) = match std::fs::read(&path) {
            Ok(bytes) if bytes.contains(&b'\r') => (HoldReason::LineEndings, Some(bytes)),
            Ok(bytes) => (HoldReason::ExternalChange, Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (HoldReason::Deleted, None),
            Err(_) => (HoldReason::ExternalChange, None),
        };
        *self.ivars().last_known.borrow_mut() = known;
        self.set_hold(Some(hold));
        self.updateChangeCount(NSDocumentChangeType::ChangeDone);
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

    /// Before an in-place write: if the file no longer holds what we last read
    /// or wrote (an external write the presenter missed), handles it as an
    /// external change now. True if it did.
    fn check_disk(&self) -> bool {
        let Some(path) = self.path() else { return false };
        if self.hold().is_some() {
            return false;
        }
        let differs = match (&*self.ivars().last_known.borrow(), std::fs::read(&path)) {
            (None, _) => false,
            (Some(known), Ok(bytes)) => *known != bytes,
            (Some(_), Err(e)) => e.kind() == std::io::ErrorKind::NotFound,
        };
        if differs {
            self.disk_changed();
        }
        differs
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
        self.set_hold(Some(HoldReason::ExternalChange));
        let this = self.retain();
        self.sheet(
            HoldReason::ExternalChange,
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
        self.set_hold(Some(HoldReason::Deleted));
        let this = self.retain();
        self.sheet(
            HoldReason::Deleted,
            "The file was deleted or moved to the Trash.",
            "Save writes your text back to disk. Close discards it.",
            ["Save", "Close"],
            move |r| {
                if r == NSAlertFirstButtonReturn {
                    this.save_back();
                } else if r == NSAlertSecondButtonReturn {
                    this.forget_draft();
                    this.close();
                }
            },
        );
    }

    fn set_hold(&self, hold: Option<HoldReason>) {
        self.ivars().autosave_hold.set(hold);
        let status = match hold {
            None => "",
            Some(HoldReason::LineEndings) => "Auto-save paused: line endings",
            Some(HoldReason::ExternalChange) => "Auto-save paused: file changed on disk",
            Some(HoldReason::Deleted) => "Auto-save paused: file deleted",
        };
        if let Some(footer) = self.editor().and_then(|e| e.footer()) {
            footer.set_paused(status);
        }
    }

    /// Writes the text to a recovery draft in ~/Library/Autosave Information
    /// (AppKit's autosave-elsewhere), never to the file itself.
    fn write_draft(&self, done: impl Fn(*mut NSError) + 'static) {
        // A fresh name each time: rewriting the existing draft makes NSDocument
        // refuse with "changed by another application" once the file changed.
        let (Some(url), Some(kind)) = (self.new_draft_url(), self.fileType()) else {
            return done(ptr::null_mut());
        };
        let replaced = self.autosavedContentsFileURL().and_then(|u| u.path()).map(|p| p.to_string());
        let this = self.retain();
        let done = RcBlock::new(move |err: *mut NSError| {
            if err.is_null() {
                guarded(|| {
                    // Keep the newest two: the previous one goes only now.
                    if let Some(oldest) = this.ivars().previous_draft.replace(replaced.clone()) {
                        this.remove_draft(&oldest);
                    }
                    this.invalidateRestorableState();
                });
            }
            done(err);
        });
        self.saveToURL_ofType_forSaveOperation_completionHandler(&url, &kind, NSSaveOperationType::AutosaveElsewhereOperation, &done);
    }

    fn new_draft_url(&self) -> Option<Retained<NSURL>> {
        let dir = drafts_dir_url(true)?;
        let unique = NSProcessInfo::processInfo().globallyUniqueString();
        let name = format!("{} (Autosaved {unique}).md", self.displayName());
        Some(dir.URLByAppendingPathComponent(&NSString::from_str(&name))?)
    }

    /// Deletes the recovery drafts once the file holds the truth again.
    fn forget_draft(&self) {
        if let Some(path) = self.autosavedContentsFileURL().and_then(|u| u.path()) {
            self.remove_draft(&path.to_string());
            self.setAutosavedContentsFileURL(None);
        }
        if let Some(path) = self.ivars().previous_draft.take() {
            self.remove_draft(&path);
        }
        self.invalidateRestorableState();
    }

    /// Removes `path` only if it is a draft: inside Autosave Information and not the file itself.
    fn remove_draft(&self, path: &str) {
        let in_drafts = autosave_dir().is_some_and(|dir| path.starts_with(&format!("{dir}/")));
        if in_drafts && self.path().as_deref() != Some(path) {
            let _ = std::fs::remove_file(path);
        } else {
            eprintln!("prasasti: not removing {path}: not a recovery draft");
        }
    }

    /// Saves to the (deleted) file's own URL. `saveDocument:` would instead ask
    /// for a new location, as AppKit thinks it lost track of the file.
    fn save_back(&self) {
        let (Some(url), Some(kind)) = (self.fileURL(), self.fileType()) else { return };
        let this = self.retain();
        let done = RcBlock::new(move |err: *mut NSError| {
            // SAFETY: the handler's error is nil or a valid NSError.
            if let Some(err) = unsafe { err.as_ref() } {
                guarded(|| {
                    // The hold stays and the text is untouched; tell the user.
                    if let Some(win) = this.windowControllers().firstObject().and_then(|c| c.window()) {
                        NSAlert::alertWithError(err, this.mtm()).beginSheetModalForWindow_completionHandler(&win, None);
                    }
                });
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
        self.forget_draft();
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
            self.set_hold(None);
            // NSDocument drops the current draft itself; the previous one is ours.
            self.forget_draft();
        }
        let writes_file = matches!(op, Op::SaveOperation | Op::SaveAsOperation | Op::AutosaveInPlaceOperation | Op::AutosaveAsOperation);
        if writes_file && pending.is_some() {
            *self.ivars().last_known.borrow_mut() = pending;
            // A write landing just after ours may never reach the presenter.
            // SAFETY: `prasastiRecheckDisk` is a no-argument void method of ours.
            unsafe {
                let _: () = msg_send![self, performSelector: sel!(prasastiRecheckDisk), withObject: None::<&AnyObject>, afterDelay: 1.0f64];
            }
        }
    }

    fn path(&self) -> Option<String> {
        Some(self.fileURL()?.path()?.to_string())
    }

    /// Shows our sheet for `reason`, replacing a stale one of ours for another reason.
    fn sheet(&self, reason: HoldReason, message: &str, info: &str, buttons: [&str; 2], on: impl Fn(NSModalResponse) + 'static) {
        if !SHEETS_ENABLED.load(Ordering::SeqCst) {
            return;
        }
        // Our own window only: a closed document must never borrow another's.
        let Some(win) = self.windowControllers().firstObject().and_then(|c| c.window()) else { return };
        let ivars = self.ivars();
        let open = ivars.sheet.borrow().clone().filter(|(_, w)| win.attachedSheet().is_some_and(|a| a == *w));
        if let Some((open_reason, open_window)) = open {
            if open_reason == reason {
                return; // already asking this
            }
            // Retire it first, so its handler ignores the end below.
            ivars.sheet_gen.set(ivars.sheet_gen.get() + 1);
            win.endSheet(&open_window);
        }
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(info));
        for b in buttons {
            alert.addButtonWithTitle(&NSString::from_str(b));
        }
        ivars.sheet_gen.set(ivars.sheet_gen.get() + 1);
        let generation = ivars.sheet_gen.get();
        *ivars.sheet.borrow_mut() = Some((reason, alert.window()));
        let this = self.retain();
        let done = RcBlock::new(move |r: NSModalResponse| {
            if this.ivars().sheet_gen.get() == generation {
                *this.ivars().sheet.borrow_mut() = None;
                guarded(|| on(r));
            }
        });
        // Another app sheet (a save panel, say) queues ours behind it.
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

    fn build_print_operation(&self, settings: &NSDictionary<NSString, AnyObject>) -> Option<Retained<NSPrintOperation>> {
        let doc: &NSDocument = self;
        let info: Retained<NSPrintInfo> = doc.printInfo().copy();
        // SAFETY: the dictionary is mutable and `settings` holds print attributes.
        unsafe { info.dictionary().addEntriesFromDictionary(settings) };
        let width = info.paperSize().width - info.leftMargin() - info.rightMargin();

        let html = crate::export::print_html(&self.text().to_string(), &crate::settings::font_family());
        let data = NSData::with_bytes(html.as_bytes());
        // SAFETY: no document-attributes out-parameter.
        let rendered = unsafe { NSAttributedString::initWithHTML_documentAttributes(NSAttributedString::alloc(), &data, None) }?;

        let mtm = MainThreadMarker::from(self);
        let storage = NSTextStorage::new();
        let layout = NSLayoutManager::new();
        storage.addLayoutManager(&layout);
        let container = NSTextContainer::initWithSize(NSTextContainer::alloc(), NSSize::new(width, f64::MAX));
        container.setWidthTracksTextView(true);
        layout.addTextContainer(&container);
        let view = NSTextView::initWithFrame_textContainer(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 1.0)),
            Some(&container),
        );
        view.setEditable(false);
        view.setHorizontallyResizable(false);
        view.setVerticallyResizable(true);
        view.setMinSize(NSSize::new(width, 1.0));
        view.setMaxSize(NSSize::new(width, f64::MAX));
        view.setBackgroundColor(&NSColor::whiteColor());
        storage.setAttributedString(&rendered);
        // HTML leaves default-black runs without a colour, and the default text colour follows Dark Mode.
        black_where_unset(&storage);
        // Lay out to the full height now, so pagination sees the real length.
        layout.ensureLayoutForTextContainer(&container);
        view.sizeToFit();
        Some(NSPrintOperation::printOperationWithView_printInfo(&view, &info))
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

fn drafts_dir_url(create: bool) -> Option<Retained<NSURL>> {
    NSFileManager::defaultManager()
        .URLForDirectory_inDomain_appropriateForURL_create_error(
            NSSearchPathDirectory::AutosavedInformationDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            None,
            create,
        )
        .ok()
}

fn autosave_dir() -> Option<String> {
    Some(drafts_dir_url(false)?.path()?.to_string())
}

/// Whether `url` lies in the Trash of its volume.
fn in_trash(url: &NSURL) -> bool {
    let trash = NSFileManager::defaultManager()
        .URLForDirectory_inDomain_appropriateForURL_create_error(
            NSSearchPathDirectory::TrashDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            Some(url),
            false,
        )
        .ok()
        .and_then(|t| t.path());
    match (trash, url.path()) {
        (Some(t), Some(p)) => p.to_string().starts_with(&format!("{t}/")),
        _ => false,
    }
}

fn internal_error() -> Retained<NSError> {
    error_with("Praśasti hit an internal error while saving. Your text is still open; try File > Save.")
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
    error_with(&format!("Auto-save is paused because {why}. Choose File > Save to keep your changes."))
}

/// User-cancelled: AppKit shows no alert for it.
fn cancelled_error() -> Retained<NSError> {
    unsafe { NSError::errorWithDomain_code_userInfo(NSCocoaErrorDomain, NSUserCancelledError, None) }
}

fn error_with(msg: &str) -> Retained<NSError> {
    let info = NSDictionary::<NSString, AnyObject>::from_slices(
        &[unsafe { NSLocalizedDescriptionKey }],
        &[&*NSString::from_str(msg) as &AnyObject],
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

fn black_where_unset(storage: &NSTextStorage) {
    // SAFETY: the key is AppKit's constant; every range comes from `attribute:atIndex:effectiveRange:`.
    unsafe {
        let key = NSForegroundColorAttributeName;
        let black = NSColor::blackColor();
        let mut at = 0;
        while at < storage.length() {
            let mut run = NSRange::new(0, 0);
            if storage.attribute_atIndex_effectiveRange(key, at, &mut run).is_none() {
                storage.addAttribute_value_range(key, &black, run);
            }
            at = run.end().max(at + 1);
        }
    }
}

fn print_error() -> Retained<NSError> {
    let info = NSDictionary::<NSString, AnyObject>::from_slices(
        &[unsafe { NSLocalizedDescriptionKey }],
        &[&**ns_string!("The document couldn't be prepared for printing.")],
    );
    unsafe { NSError::errorWithDomain_code_userInfo(NSCocoaErrorDomain, NSFeatureUnsupportedError, Some(&info)) }
}

fn set_error(out: *mut *mut NSError, err: Retained<NSError>) {
    if !out.is_null() {
        // SAFETY: Cocoa's NSError** convention; the caller owns an autorelease pool.
        unsafe { *out = Retained::autorelease_ptr(err) };
    }
}
