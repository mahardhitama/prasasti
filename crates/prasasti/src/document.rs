use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSDocument, NSScrollView, NSWindow,
    NSWindowController, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSCocoaErrorDomain, NSData, NSDictionary, NSError,
    NSFileReadInapplicableStringEncodingError, NSLocalizedDescriptionKey, NSObjectProtocol,
    NSPoint, NSRect, NSSize, NSString, ns_string,
};
use prasasti_core::file::{decode, normalize_newlines, saves_unchanged};

use crate::editor::{EditorView, make_editor};
use crate::util::guarded;

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

            let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
            scroll.setHasVerticalScroller(true);
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            let editor = make_editor(NSRect::new(NSPoint::new(0.0, 0.0), scroll.contentSize()));
            editor.setString(&self.ivars().text.borrow());
            scroll.setDocumentView(Some(&editor));
            window.setContentView(Some(&scroll));
            window.makeFirstResponder(Some(&editor));

            let controller = NSWindowController::initWithWindow(
                NSWindowController::alloc(mtm),
                Some(&window),
            );
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
            NSData::with_bytes(&bytes)
        }
    }
);

impl PrasastiDocument {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            autosave_hold: Cell::new(None),
            last_known: RefCell::new(None),
            text: RefCell::new(NSString::new()),
            editor: RefCell::new(None),
        });
        // SAFETY: NSDocument's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// The current text: the editor's when a window exists, else the loaded text.
    pub fn text(&self) -> Retained<NSString> {
        match &*self.ivars().editor.borrow() {
            Some(editor) => editor.string(),
            None => self.ivars().text.borrow().clone(),
        }
    }

    pub fn hold(&self) -> Option<HoldReason> {
        self.ivars().autosave_hold.get()
    }

    pub fn editor(&self) -> Option<Retained<EditorView>> {
        self.ivars().editor.borrow().clone()
    }
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
