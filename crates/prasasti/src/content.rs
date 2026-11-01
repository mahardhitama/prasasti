//! The document window's content view: the editor's scroll view fills the
//! window (under the transparent titlebar), the find bar sits below the
//! titlebar, and the status pill floats near the bottom.

use std::cell::{OnceCell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControlSize, NSLayoutConstraint, NSResponder, NSScreen, NSScrollView, NSScroller, NSView,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSRect};

use crate::editor::{EditorView, editor_in_scroll};
use crate::find_bar::FindBar;
use crate::settings;
use crate::status::StatusPill;
use crate::util::{Observer, guarded, observe};

/// Room left at the bottom of the text for the floating toolbar.
const BOTTOM_INSET: f64 = 64.0;
/// The editor's horizontal inset on each side at the minimum width.
const SIDE: f64 = 16.0;

pub struct Ivars {
    editor: Retained<EditorView>,
    scroll: Retained<NSScrollView>,
    find_bar: Retained<FindBar>,
    status: Retained<StatusPill>,
    /// The scroll view's top: (the view's top, while the find bar is hidden;
    /// the bar's bottom, while it shows).
    tops: OnceCell<[Retained<NSLayoutConstraint>; 2]>,
    observer: RefCell<Option<Observer>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements we violate; no Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct DocumentView;

    unsafe impl NSObjectProtocol for DocumentView {}

    impl DocumentView {
        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            // SAFETY: NSView's own implementation, same signature.
            let () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
            guarded(|| self.update_min_width());
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: NSView's own implementation, same signature.
            let () = unsafe { msg_send![super(self), layout] };
            guarded(|| self.apply_insets());
        }
    }
);

impl DocumentView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let (scroll, editor) = editor_in_scroll(frame);
        let find_bar = FindBar::new(mtm, &editor);
        let status = StatusPill::new(mtm);
        editor.set_status_pill(&status);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            editor,
            scroll,
            find_bar,
            status,
            tops: OnceCell::new(),
            observer: RefCell::default(),
        });
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let iv = this.ivars();
        for v in [&*iv.scroll as &NSView, &iv.find_bar, &iv.status] {
            v.setTranslatesAutoresizingMaskIntoConstraints(false);
            this.addSubview(v);
        }
        let tops = [
            iv.scroll.topAnchor().constraintEqualToAnchor(&this.topAnchor()),
            iv.scroll.topAnchor().constraintEqualToAnchor(&iv.find_bar.bottomAnchor()),
        ];
        tops[0].setActive(true);
        let _ = iv.tops.set(tops);
        let safe = this.safeAreaLayoutGuide();
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            iv.find_bar.topAnchor().constraintEqualToAnchor(&safe.topAnchor()),
            iv.find_bar.leadingAnchor().constraintEqualToAnchor(&this.leadingAnchor()),
            iv.find_bar.trailingAnchor().constraintEqualToAnchor(&this.trailingAnchor()),
            iv.scroll.leadingAnchor().constraintEqualToAnchor(&this.leadingAnchor()),
            iv.scroll.trailingAnchor().constraintEqualToAnchor(&this.trailingAnchor()),
            iv.scroll.bottomAnchor().constraintEqualToAnchor(&this.bottomAnchor()),
            iv.status.centerXAnchor().constraintEqualToAnchor(&this.centerXAnchor()),
            this.bottomAnchor().constraintEqualToAnchor_constant(&iv.status.bottomAnchor(), 56.0),
        ]));
        iv.scroll.setAutomaticallyAdjustsContentInsets(false);
        this.apply_insets();
        let weak = Weak::new(&*this);
        *iv.observer.borrow_mut() = Some(observe(settings::CHANGED, None, move || {
            if let Some(v) = weak.load() {
                v.update_min_width();
            }
        }));
        this
    }

    pub fn editor(&self) -> Retained<EditorView> {
        self.ivars().editor.clone()
    }

    pub fn status(&self) -> Retained<StatusPill> {
        self.ivars().status.clone()
    }

    /// Puts the text below the find bar while it shows; called when it shows or hides.
    pub fn find_bar_toggled(&self) {
        let iv = self.ivars();
        let shown = !iv.find_bar.isHidden();
        if let Some([full, bar]) = iv.tops.get() {
            full.setActive(!shown);
            bar.setActive(shown);
        }
        self.apply_insets();
    }

    /// The text clears the titlebar (when the scroll view runs under it) and the toolbar.
    fn apply_insets(&self) {
        let iv = self.ivars();
        let top = if iv.find_bar.isHidden() { self.safeAreaInsets().top } else { 0.0 };
        let insets = NSEdgeInsets { top, left: 0.0, bottom: BOTTOM_INSET, right: 0.0 };
        let cur = iv.scroll.contentInsets();
        if (cur.top, cur.bottom) != (insets.top, insets.bottom) {
            iv.scroll.setContentInsets(insets);
        }
    }

    /// Sets the window's minimum content width to fit the column (capped at
    /// the screen), and widens the window if it is narrower.
    pub fn update_min_width(&self) {
        let Some(win) = self.window() else { return };
        let scroller = NSScroller::scrollerWidthForControlSize_scrollerStyle(
            NSControlSize::Regular,
            self.ivars().scroll.scrollerStyle(),
            self.mtm(),
        );
        let mut min = self.ivars().editor.column_pts() + 2.0 * SIDE + scroller;
        let visible = win.screen().or_else(|| NSScreen::mainScreen(self.mtm())).map(|s| s.visibleFrame());
        if let Some(v) = visible {
            min = min.min(v.size.width);
        }
        let mut size = win.contentMinSize();
        size.width = min;
        win.setContentMinSize(size);
        let mut frame = win.frame();
        let content = win.contentRectForFrameRect(frame).size.width;
        if content < min {
            // Grow from the left edge, then slide back onto the screen.
            frame.size.width += min - content;
            if let Some(v) = visible {
                let right = v.origin.x + v.size.width;
                if frame.origin.x + frame.size.width > right {
                    frame.origin.x = (right - frame.size.width).max(v.origin.x);
                }
            }
            win.setFrame_display(frame, true);
        }
    }
}
