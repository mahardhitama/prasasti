use std::cell::{Cell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSButton, NSFontManager, NSLayoutConstraintOrientation, NSResponder, NSStackView, NSTextField, NSView,
};
use objc2_foundation::{MainThreadMarker, NSEdgeInsets, NSObject, NSObjectProtocol, NSRect, NSString, NSTimer};
use prasasti_core::words::count_words;

use crate::editor::EditorView;
use crate::util::{guarded, to_u16};

const WORD_DELAY: f64 = 0.3;
const STATUS_SECS: f64 = 3.0;

pub struct Ivars {
    editor: Weak<EditorView>,
    words_label: Retained<NSTextField>,
    status_label: Retained<NSTextField>,
    words: Cell<usize>,
    word_timer: RefCell<Option<Retained<NSTimer>>>,
    status_timer: RefCell<Option<Retained<NSTimer>>>,
    /// A status that stays until replaced (autosave paused); 3 s messages show over it.
    paused: RefCell<String>,
}

define_class!(
    /// The row below the editor: word count, a status message, the font button.
    // SAFETY: NSStackView has no subclassing requirements we violate; no Drop.
    #[unsafe(super(NSStackView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct Footer;

    unsafe impl NSObjectProtocol for Footer {}

    impl Footer {
        #[unsafe(method(countWords:))]
        fn count_words_action(&self, _timer: &NSTimer) {
            let Some(editor) = self.ivars().editor.load() else { return };
            let text = to_u16(&editor.string());
            if let Some(n) = guarded(|| count_words(&text)) {
                self.set_words(n);
            }
        }

        #[unsafe(method(clearStatus:))]
        fn clear_status(&self, _timer: &NSTimer) {
            self.ivars().status_timer.borrow_mut().take();
            self.ivars().status_label.setStringValue(&NSString::from_str(&self.ivars().paused.borrow()));
        }
    }
);

impl Footer {
    /// A footer for `editor`, which it registers with.
    pub fn new(mtm: MainThreadMarker, editor: &EditorView) -> Retained<Self> {
        let label = || NSTextField::labelWithString(&NSString::new(), mtm);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            editor: Weak::new(editor),
            words_label: label(),
            status_label: label(),
            words: Cell::new(0),
            word_timer: RefCell::default(),
            status_timer: RefCell::default(),
            paused: RefCell::default(),
        });
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        // SAFETY: the shared font manager lives for the whole app and has
        // `-(void)orderFrontFontPanel:(id)sender`.
        let font = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Font"),
                Some(&NSFontManager::sharedFontManager(mtm)),
                Some(sel!(orderFrontFontPanel:)),
                mtm,
            )
        };
        let ivars = this.ivars();
        // The status takes the slack, pushing the button to the right edge.
        ivars.status_label.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
        for v in [&*ivars.words_label as &NSView, &ivars.status_label, &font] {
            this.addArrangedSubview(v);
        }
        this.setEdgeInsets(NSEdgeInsets { top: 4.0, left: 8.0, bottom: 4.0, right: 8.0 });
        this.setHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Vertical);
        this.set_words(0);
        editor.set_footer(&this);
        this
    }

    /// Recounts 300 ms after the last call.
    pub fn text_changed(&self) {
        self.restart(&self.ivars().word_timer, WORD_DELAY, sel!(countWords:));
    }

    pub fn set_words(&self, n: usize) {
        self.ivars().words.set(n);
        let unit = if n == 1 { "Word" } else { "Words" };
        self.ivars().words_label.setStringValue(&NSString::from_str(&format!("{n} {unit}")));
    }

    /// Shows `s` for 3 s.
    pub fn set_status(&self, s: &str) {
        self.ivars().status_label.setStringValue(&NSString::from_str(s));
        self.restart(&self.ivars().status_timer, STATUS_SECS, sel!(clearStatus:));
    }

    /// Shows `s` until the next call ("" clears it); a 3 s message shows over it.
    pub fn set_paused(&self, s: &str) {
        *self.ivars().paused.borrow_mut() = s.to_string();
        if self.ivars().status_timer.borrow().is_none() {
            self.ivars().status_label.setStringValue(&NSString::from_str(s));
        }
    }

    pub fn words(&self) -> usize {
        self.ivars().words.get()
    }

    pub fn words_label(&self) -> String {
        self.ivars().words_label.stringValue().to_string()
    }

    pub fn status(&self) -> String {
        self.ivars().status_label.stringValue().to_string()
    }

    /// Replaces the pending one-shot timer in `slot` with a new one.
    fn restart(&self, slot: &RefCell<Option<Retained<NSTimer>>>, secs: f64, action: objc2::runtime::Sel) {
        if let Some(t) = slot.borrow_mut().take() {
            t.invalidate();
        }
        // SAFETY: `action` is a `-(void)x:(NSTimer *)t` method of the footer;
        // the timer retains the footer until it fires or is invalidated.
        let t = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                secs,
                self,
                action,
                None::<&AnyObject>,
                false,
            )
        };
        *slot.borrow_mut() = Some(t);
    }
}
