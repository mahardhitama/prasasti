//! The document window's content view: the editor's scroll view fills the
//! window (under the transparent titlebar), the find bar sits below the
//! titlebar, and the status pill floats near the bottom, above the toolbar.
//! It also owns the window's mode: Edit shows the editor, Preview hides it
//! (it stays in the window, the only source of truth) and shows a web view.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSDocument, NSLayoutConstraint, NSMenuItem, NSResponder,
    NSScreen, NSScrollView, NSScroller, NSTextStorageDidProcessEditingNotification, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSRect, NSTimer};

use crate::editor::{EditorView, editor_in_scroll};
use crate::find_bar::FindBar;
use crate::preview::{self, PreviewView};
use crate::settings;
use crate::status::StatusPill;
use crate::toolbar::Toolbar;
use crate::util::{Observer, guarded, observe};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Preview,
    Edit,
}

/// Preview re-renders at most this often while the text changes underneath it.
const RENDER_DELAY: f64 = 0.25;

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
    observers: RefCell<Vec<Observer>>,
    toolbar: Toolbar,
    mode: Cell<Mode>,
    /// Created on the first switch to Preview.
    web: OnceCell<Retained<PreviewView>>,
    /// A pending Preview re-render.
    timer: RefCell<Option<Retained<NSTimer>>>,
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

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn view_did_change_effective_appearance(&self) {
            // SAFETY: NSView's own implementation, same signature.
            let () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            guarded(|| self.ivars().toolbar.apply_appearance());
        }
        #[unsafe(method(showPreview:))]
        fn show_preview(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.set_mode(Mode::Preview));
        }
        #[unsafe(method(showEditor:))]
        fn show_editor(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.set_mode(Mode::Edit));
        }
        #[unsafe(method(togglePreview:))]
        fn toggle_preview(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.set_mode(if self.mode() == Mode::Preview { Mode::Edit } else { Mode::Preview }));
        }

        // Before the window's own undo:/redo:, so Preview can refuse them.
        #[unsafe(method(undo:))]
        fn undo(&self, _sender: Option<&AnyObject>) {
            guarded(|| {
                if let Some(u) = self.undoManager().filter(|_| self.mode() == Mode::Edit) {
                    u.undo();
                }
            });
        }
        #[unsafe(method(redo:))]
        fn redo(&self, _sender: Option<&AnyObject>) {
            guarded(|| {
                if let Some(u) = self.undoManager().filter(|_| self.mode() == Mode::Edit) {
                    u.redo();
                }
            });
        }

        // The editor handles these itself in Edit; here only in Preview.
        #[unsafe(method(showFindBar:))]
        fn show_find_bar(&self, sender: Option<&AnyObject>) {
            guarded(|| {
                self.set_mode(Mode::Edit);
                // SAFETY: the editor's own action, same signature.
                let () = unsafe { msg_send![&*self.ivars().editor, showFindBar: sender] };
            });
        }
        #[unsafe(method(showFindReplaceBar:))]
        fn show_find_replace_bar(&self, sender: Option<&AnyObject>) {
            guarded(|| {
                self.set_mode(Mode::Edit);
                // SAFETY: the editor's own action, same signature.
                let () = unsafe { msg_send![&*self.ivars().editor, showFindReplaceBar: sender] };
            });
        }
        #[unsafe(method(zoomIn:))]
        fn zoom_in(&self, sender: Option<&AnyObject>) {
            // SAFETY: the editor's own action, same signature.
            guarded(|| -> () { unsafe { msg_send![&*self.ivars().editor, zoomIn: sender] } });
        }
        #[unsafe(method(zoomOut:))]
        fn zoom_out(&self, sender: Option<&AnyObject>) {
            // SAFETY: the editor's own action, same signature.
            guarded(|| -> () { unsafe { msg_send![&*self.ivars().editor, zoomOut: sender] } });
        }
        #[unsafe(method(resetZoom:))]
        fn reset_zoom(&self, sender: Option<&AnyObject>) {
            // SAFETY: the editor's own action, same signature.
            guarded(|| -> () { unsafe { msg_send![&*self.ivars().editor, resetZoom: sender] } });
        }
        #[unsafe(method(copyAsHTML:))]
        fn copy_as_html(&self, sender: Option<&AnyObject>) {
            // SAFETY: the editor's own action, same signature.
            guarded(|| -> () { unsafe { msg_send![&*self.ivars().editor, copyAsHTML: sender] } });
        }

        #[unsafe(method(renderPreview:))]
        fn render_preview(&self, _timer: &NSTimer) {
            guarded(|| {
                self.ivars().timer.borrow_mut().take();
                if self.mode() == Mode::Preview {
                    self.render();
                }
            });
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            guarded(|| self.validate(item)).unwrap_or(false)
        }
    }
);

impl DocumentView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let (scroll, editor) = editor_in_scroll(frame);
        let find_bar = FindBar::new(mtm, &editor);
        let status = StatusPill::new(mtm);
        editor.set_status_pill(&status);
        let toolbar = Toolbar::new(mtm);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            editor,
            scroll,
            find_bar,
            status,
            tops: OnceCell::new(),
            observers: RefCell::default(),
            toolbar,
            mode: Cell::new(Mode::Edit),
            web: OnceCell::new(),
            timer: RefCell::default(),
        });
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let iv = this.ivars();
        iv.toolbar.set_target(&this);
        for v in [&*iv.scroll as &NSView, &iv.find_bar, &iv.status, iv.toolbar.view()] {
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
            iv.toolbar.view().centerXAnchor().constraintEqualToAnchor(&this.centerXAnchor()),
            this.bottomAnchor().constraintEqualToAnchor_constant(&iv.toolbar.view().bottomAnchor(), 12.0),
        ]));
        iv.scroll.setAutomaticallyAdjustsContentInsets(false);
        this.apply_insets();
        let weak = Weak::new(&*this);
        let settings_changed = observe(settings::CHANGED, None, move || {
            if let Some(v) = weak.load() {
                v.update_min_width();
                if let Some(web) = v.ivars().web.get() {
                    // SAFETY: a plain setter; zoom is 0.5...3.0.
                    unsafe { web.setPageZoom(settings::zoom()) };
                }
                // The font or column may have changed (an unchanged page is not reloaded).
                if v.mode() == Mode::Preview {
                    v.render();
                }
            }
        });
        let weak = Weak::new(&*this);
        let storage: &AnyObject = iv.editor.storage();
        // SAFETY: a constant string.
        let name = unsafe { NSTextStorageDidProcessEditingNotification }.to_string();
        let text_changed = observe(&name, Some(storage), move || {
            if let Some(v) = weak.load() {
                v.text_changed();
            }
        });
        *iv.observers.borrow_mut() = vec![settings_changed, text_changed];
        this
    }

    pub fn mode(&self) -> Mode {
        self.ivars().mode.get()
    }

    /// Shows the editor or the preview (rendered now) and makes it first responder.
    pub fn set_mode(&self, m: Mode) {
        let iv = self.ivars();
        iv.mode.set(m);
        iv.toolbar.set_selected(m);
        let preview = m == Mode::Preview;
        if preview {
            // Its Replace would edit the hidden text.
            if !iv.find_bar.isHidden() {
                iv.find_bar.hide();
            }
            self.render();
        }
        iv.scroll.setHidden(preview);
        let web = iv.web.get();
        if let Some(w) = web {
            w.setHidden(!preview);
        }
        if let Some(win) = self.window() {
            let first: &NSResponder = match web {
                Some(w) if preview => w,
                _ => &iv.editor,
            };
            win.makeFirstResponder(Some(first));
        }
    }

    pub fn toolbar(&self) -> &Toolbar {
        &self.ivars().toolbar
    }

    /// The last page handed to the web view (None before the first Preview).
    pub fn preview_html(&self) -> Option<String> {
        self.ivars().web.get().map(|w| w.html())
    }

    /// The Preview web view (None before the first Preview).
    pub fn preview(&self) -> Option<Retained<PreviewView>> {
        self.ivars().web.get().cloned()
    }

    /// Renders the text storage into the web view (never writes to it).
    fn render(&self) {
        let iv = self.ivars();
        let text = iv.editor.string().to_string();
        let html = preview::page_html(&text, &settings::font_family(), settings::BASE_SIZE, settings::column_width());
        let web = iv.web.get_or_init(|| self.make_web());
        web.render(&html, &preview::base_url(self.doc_dir().as_deref()));
    }

    /// The document's folder, where relative image paths resolve (None when untitled).
    fn doc_dir(&self) -> Option<std::path::PathBuf> {
        // SAFETY: a plain getter; the document outlives its window's content.
        let doc = unsafe { self.window()?.windowController()?.document() }?;
        let path = doc.downcast::<NSDocument>().ok()?.fileURL()?.path()?.to_string();
        Some(std::path::Path::new(&path).parent()?.to_path_buf())
    }

    fn make_web(&self) -> Retained<PreviewView> {
        let web = PreviewView::new(self.mtm(), self.bounds(), &self.ivars().editor);
        web.setTranslatesAutoresizingMaskIntoConstraints(false);
        // Under the find bar, status pill and toolbar.
        self.addSubview_positioned_relativeTo(&web, NSWindowOrderingMode::Below, Some(&self.ivars().scroll));
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            web.topAnchor().constraintEqualToAnchor(&self.topAnchor()),
            web.bottomAnchor().constraintEqualToAnchor(&self.bottomAnchor()),
            web.leadingAnchor().constraintEqualToAnchor(&self.leadingAnchor()),
            web.trailingAnchor().constraintEqualToAnchor(&self.trailingAnchor()),
        ]));
        // SAFETY: a plain setter; zoom is 0.5...3.0.
        unsafe { web.setPageZoom(settings::zoom()) };
        // SAFETY: a plain setter.
        unsafe { web.setObscuredContentInsets(NSEdgeInsets { top: self.safeAreaInsets().top, left: 0.0, bottom: 0.0, right: 0.0 }) };
        web
    }

    /// The text storage changed: in Preview, re-render once RENDER_DELAY passes.
    fn text_changed(&self) {
        let iv = self.ivars();
        if self.mode() != Mode::Preview || iv.timer.borrow().is_some() {
            return;
        }
        // SAFETY: `renderPreview:` is a `-(void)x:(NSTimer *)t` method of this
        // view; the timer retains the view until it fires.
        let t = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                RENDER_DELAY,
                self,
                sel!(renderPreview:),
                None::<&AnyObject>,
                false,
            )
        };
        *iv.timer.borrow_mut() = Some(t);
    }

    /// The active mode's item is checked; undo and redo are off in Preview.
    fn validate(&self, item: &NSMenuItem) -> bool {
        let Some(action) = item.action() else { return true };
        let preview = self.mode() == Mode::Preview;
        if action == sel!(showPreview:) || action == sel!(showEditor:) {
            let on = (action == sel!(showPreview:)) == preview;
            item.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        } else if action == sel!(undo:) || action == sel!(redo:) {
            let Some(um) = self.undoManager().filter(|_| !preview) else { return false };
            return if action == sel!(undo:) {
                item.setTitle(&um.undoMenuItemTitle());
                um.canUndo()
            } else {
                item.setTitle(&um.redoMenuItemTitle());
                um.canRedo()
            };
        }
        true
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
            // Deactivate first: both active at once would conflict.
            let (off, on) = if shown { (full, bar) } else { (bar, full) };
            off.setActive(false);
            on.setActive(true);
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
        if let Some(web) = iv.web.get() {
            // The page's first lines clear the titlebar and toolbar.
            let top = self.safeAreaInsets().top;
            // SAFETY: plain getter and setter.
            if unsafe { web.obscuredContentInsets() }.top != top {
                unsafe { web.setObscuredContentInsets(NSEdgeInsets { top, left: 0.0, bottom: 0.0, right: 0.0 }) };
            }
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
