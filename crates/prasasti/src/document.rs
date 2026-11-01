use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSDocument, NSLayoutAttribute, NSStackView,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowController, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSCocoaErrorDomain, NSCopying, NSData, NSDictionary, NSError,
    NSFileReadInapplicableStringEncodingError, NSLocalizedDescriptionKey, NSObjectProtocol,
    NSPoint, NSRect, NSSize, NSString, ns_string,
};
use prasasti_core::file::{decode, normalize_newlines, saves_unchanged};

use crate::editor::{EditorView, editor_in_scroll};
use crate::find_bar::FindBar;
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
            // A copy: the editor's string is its live, mutable backing store.
            Some(editor) => editor.string().copy(),
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

/// The window's content, top to bottom: the (hidden) find bar, then the editor.
pub fn window_content(frame: NSRect) -> (Retained<NSStackView>, Retained<EditorView>) {
    let mtm = MainThreadMarker::new().expect("windows are built on the main thread");
    let (scroll, editor) = editor_in_scroll(frame);
    let bar = FindBar::new(mtm, &editor);
    let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(&[&*bar as &NSView, &scroll]), mtm);
    stack.setFrame(frame);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Width);
    stack.setSpacing(0.0);
    (stack, editor)
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
