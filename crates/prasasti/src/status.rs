//! The status pill: a glass capsule above the toolbar showing "Auto-save
//! paused: …" for as long as autosave is held, and 2 s messages over it.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibilityAnnouncementKey, NSAccessibilityAnnouncementRequestedNotification, NSAccessibilityPostNotificationWithUserInfo,
    NSAccessibilityPriorityKey, NSAccessibilityPriorityLevel, NSGlassEffectView, NSResponder, NSTextField, NSView,
};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSRect, NSString, NSTimer};

use crate::util::guarded;

const FLASH_SECS: f64 = 2.0;

pub struct Ivars {
    label: Retained<NSTextField>,
    /// The persistent text (autosave paused); flashes show over it.
    paused: RefCell<Option<String>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

define_class!(
    // SAFETY: NSGlassEffectView has no subclassing requirements we violate; no Drop.
    #[unsafe(super(NSGlassEffectView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct StatusPill;

    unsafe impl NSObjectProtocol for StatusPill {}

    impl StatusPill {
        #[unsafe(method(endFlash:))]
        fn end_flash(&self, _timer: &NSTimer) {
            guarded(|| {
                self.ivars().timer.borrow_mut().take();
                self.show(self.ivars().paused.borrow().as_deref());
            });
        }
    }
);

impl StatusPill {
    /// A hidden pill.
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let label = NSTextField::labelWithString(&NSString::new(), mtm);
        let this = Self::alloc(mtm).set_ivars(Ivars { label, paused: RefCell::default(), timer: RefCell::default() });
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        let label = &this.ivars().label;
        this.setContentView(Some(label));
        this.setCornerRadius(14.0);
        // Padding around the text; the pill sizes to it.
        label.setTranslatesAutoresizingMaskIntoConstraints(false);
        for c in [
            label.leadingAnchor().constraintEqualToAnchor_constant(&this.leadingAnchor(), 14.0),
            this.trailingAnchor().constraintEqualToAnchor_constant(&label.trailingAnchor(), 14.0),
            label.topAnchor().constraintEqualToAnchor_constant(&this.topAnchor(), 6.0),
            this.bottomAnchor().constraintEqualToAnchor_constant(&label.bottomAnchor(), 6.0),
        ] {
            c.setActive(true);
        }
        this.setHidden(true);
        this
    }

    /// Shows `text` until the next call (None hides it); a flash shows over it.
    pub fn set_paused(&self, text: Option<&str>) {
        *self.ivars().paused.borrow_mut() = text.map(Into::into);
        if self.ivars().timer.borrow().is_none() {
            self.show(text);
        }
        if let Some(t) = text {
            self.announce(t);
        }
    }

    /// Shows `text` for 2 s, then the paused text or nothing.
    pub fn flash(&self, text: &str) {
        self.show(Some(text));
        self.announce(text);
        if let Some(t) = self.ivars().timer.borrow_mut().take() {
            t.invalidate();
        }
        // SAFETY: `endFlash:` is a `-(void)x:(NSTimer *)t` method of the pill;
        // the timer retains the pill until it fires or is invalidated.
        let t = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                FLASH_SECS,
                self,
                sel!(endFlash:),
                None::<&AnyObject>,
                false,
            )
        };
        *self.ivars().timer.borrow_mut() = Some(t);
    }

    /// The shown text, or "" when hidden.
    pub fn text(&self) -> String {
        if self.is_shown() { self.ivars().label.stringValue().to_string() } else { String::new() }
    }

    pub fn is_shown(&self) -> bool {
        !self.isHidden()
    }

    fn show(&self, text: Option<&str>) {
        self.ivars().label.setStringValue(&NSString::from_str(text.unwrap_or("")));
        self.setHidden(text.is_none());
    }

    /// Tells VoiceOver; "Auto-save paused" is data-safety news.
    fn announce(&self, text: &str) {
        let (text, priority) = (NSString::from_str(text), NSNumber::new_isize(NSAccessibilityPriorityLevel::High.0));
        // SAFETY: the keys are AppKit's constants; the values are an NSString and an NSNumber, as documented.
        unsafe {
            let info = NSDictionary::from_slices(
                &[NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
                &[&*text as &AnyObject, &*priority as &AnyObject],
            );
            NSAccessibilityPostNotificationWithUserInfo(self, NSAccessibilityAnnouncementRequestedNotification, Some(&info));
        }
    }
}
