use std::cell::{Cell, Ref, RefCell};
use std::ptr::NonNull;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSBackgroundColorAttributeName, NSFontAttributeName, NSStringDrawing, NSEvent, NSEventModifierFlags, NSFont,
    NSFontManager,
    NSForegroundColorAttributeName, NSGlyphProperty, NSLayoutManager, NSLayoutManagerDelegate,
    NSPasteboard, NSPasteboardTypeString, NSResponder, NSScrollView, NSText, NSTextContainer,
    NSTextDelegate, NSTextInputClient, NSTextStorage, NSTextStorageDelegate,
    NSTextStorageEditActions, NSTextView, NSTextViewDelegate, NSView,
};

use objc2_foundation::{
    MainThreadMarker, NSArray, NSDictionary, NSInteger, NSNotification, NSObject, NSNotFound, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString, NSUInteger,
};

use prasasti_core::edit::{self, Edit};
use prasasti_core::file::normalize_newlines;
use prasasti_core::list;
use prasasti_core::zoom;

use crate::find_bar::FindBar;
use crate::status::StatusPill;
use crate::settings;
use crate::styler::{Styler, hidden_key};
use crate::theme::{Palette, base_font, palette};
use crate::util::{Observer, guarded, observe, to_u16};

pub struct Ivars {
    /// The view does not retain its storage (the storage owns the layout
    /// manager, which owns the container), so the view keeps it alive.
    storage: Retained<NSTextStorage>,
    styler: RefCell<Styler>,
    /// The caret as of the last selection change, for marker reveal.
    caret: Cell<usize>,
    pal: RefCell<Palette>,
    /// The window's find bar, which repaints its matches after edits.
    find_bar: RefCell<Weak<FindBar>>,
    status: RefCell<Weak<StatusPill>>,
    /// The font the styler uses, from the settings as of the last restyle.
    font: RefCell<Retained<NSFont>>,
    /// ⌘-scroll lines not yet turned into a zoom step.
    wheel: Cell<f64>,
    /// Settings and clip-frame observations, removed when the editor goes.
    observers: RefCell<Vec<Observer>>,
}

define_class!(
    /// The plain-text Markdown editor: live styling, plus the Edit, Format
    /// and View menu actions (formatting, paste, Copy as HTML, zoom).
    // SAFETY: NSTextView has no subclassing requirements we violate; no Drop.
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct EditorView;

    unsafe impl NSObjectProtocol for EditorView {}

    unsafe impl NSTextStorageDelegate for EditorView {
        #[unsafe(method(textStorage:didProcessEditing:range:changeInLength:))]
        fn did_process_editing(&self, storage: &NSTextStorage, mask: NSTextStorageEditActions, range: NSRange, _delta: NSInteger) {
            // Our own attribute passes re-enter here with attributes only.
            if !mask.contains(NSTextStorageEditActions::EditedCharacters) {
                return;
            }
            let Ok(mut styler) = self.ivars().styler.try_borrow_mut() else { return };
            let pal = self.ivars().pal.borrow();
            let font = self.ivars().font.borrow();
            // The selection moves after this; the end of the edit is where it lands.
            guarded(|| styler.restyle_edit(storage, &font, &pal, range, range.end()));
            if let Some(bar) = self.find_bar() {
                guarded(|| bar.text_changed());
            }
        }
    }

    unsafe impl NSLayoutManagerDelegate for EditorView {
        /// Glyphs of characters marked hidden get `NSGlyphPropertyNull` (no space).
        #[unsafe(method(layoutManager:shouldGenerateGlyphs:properties:characterIndexes:font:forGlyphRange:))]
        unsafe fn should_generate_glyphs(
            &self,
            lm: &NSLayoutManager,
            glyphs: NonNull<u16>, // CGGlyph
            props: NonNull<NSGlyphProperty>,
            chars: NonNull<NSUInteger>,
            font: &NSFont,
            range: NSRange,
        ) -> NSUInteger {
            let n = range.length;
            // SAFETY: AppKit passes `n` properties and character indexes.
            let (props_in, chars_in) = unsafe {
                (std::slice::from_raw_parts(props.as_ptr(), n), std::slice::from_raw_parts(chars.as_ptr(), n))
            };
            let storage = &self.ivars().storage;
            let mut mine = props_in.to_vec();
            let (mut run, mut hidden, mut any) = (NSRange::new(0, 0), false, false);
            for (prop, &c) in mine.iter_mut().zip(chars_in) {
                if !(run.location <= c && c < run.end()) {
                    // SAFETY: `run` is a valid out pointer; `c` is inside the storage.
                    hidden = unsafe { storage.attribute_atIndex_effectiveRange(hidden_key(), c, &mut run) }.is_some();
                }
                if hidden {
                    *prop |= NSGlyphProperty::Null;
                    any = true;
                }
            }
            if !any {
                return 0; // 0: let the layout manager set the glyphs unchanged.
            }
            // SAFETY: all three buffers hold `n` entries for `range`.
            unsafe {
                lm.setGlyphs_properties_characterIndexes_font_forGlyphRange(
                    glyphs, NonNull::new_unchecked(mine.as_mut_ptr()), chars, font, range,
                )
            };
            n
        }
    }

    unsafe impl NSTextDelegate for EditorView {}

    unsafe impl NSTextViewDelegate for EditorView {
        #[unsafe(method(textView:willChangeSelectionFromCharacterRange:toCharacterRange:))]
        fn will_change_selection(&self, _tv: &NSTextView, old: NSRange, new: NSRange) -> NSRange {
            caret_before_hidden(self.storage(), old, new)
        }

        #[unsafe(method(textViewDidChangeSelection:))]
        fn text_view_did_change_selection(&self, _note: &NSNotification) {
            // The current-match colour follows the selection (clicks, arrows).
            if let Some(bar) = self.find_bar().filter(|b| !b.isHidden()) {
                guarded(|| bar.paint());
            }
            let new = self.selectedRange().location;
            let old = self.ivars().caret.replace(new);
            if old == new {
                return;
            }
            let Ok(mut styler) = self.ivars().styler.try_borrow_mut() else { return };
            let pal = self.ivars().pal.borrow();
            let font = self.ivars().font.borrow();
            guarded(|| styler.restyle_caret_move(self.storage(), &font, &pal, old, new));
        }
    }

    impl EditorView {
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn view_did_change_effective_appearance(&self) {
            // SAFETY: NSView's own implementation, same signature.
            let () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            self.apply_theme();
        }

        #[unsafe(method(toggleBold:))]
        fn toggle_bold(&self, _sender: Option<&AnyObject>) {
            self.emphasis("**");
        }
        #[unsafe(method(toggleItalic:))]
        fn toggle_italic(&self, _sender: Option<&AnyObject>) {
            self.emphasis("*");
        }
        #[unsafe(method(toggleStrikethrough:))]
        fn toggle_strikethrough(&self, _sender: Option<&AnyObject>) {
            self.emphasis("~~");
        }
        #[unsafe(method(insertLink:))]
        fn insert_link(&self, _sender: Option<&AnyObject>) {
            let clip = NSPasteboard::generalPasteboard()
                .stringForType(unsafe { NSPasteboardTypeString })
                .map(|s| s.to_string())
                .unwrap_or_default();
            let text = to_u16(&self.string());
            let sel = range_of(self.selectedRange());
            if let Some(e) = guarded(|| edit::insert_link(&text, sel, &clip)) {
                self.apply_edit(&e);
            }
        }

        #[unsafe(method(insertTab:))]
        fn insert_tab(&self, sender: Option<&AnyObject>) {
            if !self.indent(false) {
                // SAFETY: NSTextView's own implementation, same signature.
                let () = unsafe { msg_send![super(self), insertTab: sender] };
            }
        }
        #[unsafe(method(insertBacktab:))]
        fn insert_backtab(&self, sender: Option<&AnyObject>) {
            if !self.indent(true) {
                // SAFETY: NSTextView's own implementation, same signature.
                let () = unsafe { msg_send![super(self), insertBacktab: sender] };
            }
        }

        /// Plain-string pastes go in with LF line endings; anything else is AppKit's.
        #[unsafe(method(paste:))]
        fn paste(&self, sender: Option<&AnyObject>) {
            let pb = NSPasteboard::generalPasteboard();
            let Some(s) = pb.stringForType(unsafe { NSPasteboardTypeString }) else {
                // SAFETY: NSText's own implementation, same signature.
                let () = unsafe { msg_send![super(self), paste: sender] };
                return;
            };
            let Some(s) = guarded(|| normalize_newlines(&s.to_string())) else { return };
            // Its own undo step: insertText would coalesce with the typing around it.
            self.breakUndoCoalescing();
            // SAFETY: an NSString is a valid argument; NSNotFound means "the selection".
            unsafe { self.insertText_replacementRange(&NSString::from_str(&s), NSRange::new(NSNotFound as usize, 0)) };
            self.breakUndoCoalescing();
        }

        /// Paste and Match Style: the editor is plain text already, so it is our paste.
        #[unsafe(method(pasteAsPlainText:))]
        fn paste_as_plain_text(&self, sender: Option<&AnyObject>) {
            // SAFETY: our own `paste:` above, same signature.
            let () = unsafe { msg_send![self, paste: sender] };
        }

        #[unsafe(method(copyAsHTML:))]
        fn copy_as_html(&self, _sender: Option<&AnyObject>) {
            // The whole document, whatever is selected.
            guarded(|| {
                crate::export::copy_as_html(&NSPasteboard::generalPasteboard(), &self.string().to_string());
                if let Some(status) = self.status() {
                    status.flash("Copied as HTML.");
                }
            });
        }
        #[unsafe(method(zoomIn:))]
        fn zoom_in(&self, _sender: Option<&AnyObject>) {
            self.zoom_by(1);
        }
        #[unsafe(method(zoomOut:))]
        fn zoom_out(&self, _sender: Option<&AnyObject>) {
            self.zoom_by(-1);
        }
        #[unsafe(method(resetZoom:))]
        fn reset_zoom(&self, _sender: Option<&AnyObject>) {
            self.zoom_by(0);
        }

        /// ⌘-scroll zooms one step per accumulated line; plain scroll scrolls.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if !event.modifierFlags().contains(NSEventModifierFlags::Command) {
                // SAFETY: NSResponder's own implementation, same signature.
                let () = unsafe { msg_send![super(self), scrollWheel: event] };
                return;
            }
            // Trackpads and Magic Mice report precise deltas in points, many
            // small ones per gesture; a wheel reports whole lines. Count 10
            // points as a line (NSScrollView's default line scroll) so a
            // trackpad swipe zooms about as far as a few wheel clicks.
            let dy = event.scrollingDeltaY();
            let lines = if event.hasPreciseScrollingDeltas() { dy / 10.0 } else { dy };
            let (rest, dir) = wheel_step(self.ivars().wheel.get(), lines);
            self.ivars().wheel.set(rest);
            if dir != 0 {
                self.zoom_by(dir);
            }
        }

        /// The font panel's choice: keep its family; the size comes from the zoom.
        #[unsafe(method(changeFont:))]
        fn change_font(&self, _sender: Option<&AnyObject>) {
            guarded(|| {
                let current = self.ivars().font.borrow().clone();
                let chosen = NSFontManager::sharedFontManager(self.mtm()).convertFont(&current);
                if let Some(family) = chosen.familyName() {
                    // Posts the settings notification, which restyles every editor.
                    settings::set_font_family(&family.to_string());
                }
            });
        }
        #[unsafe(method(showFindBar:))]
        fn show_find_bar(&self, _sender: Option<&AnyObject>) {
            self.find_bar().map(|b| guarded(|| b.show(false)));
        }
        #[unsafe(method(showFindReplaceBar:))]
        fn show_find_replace_bar(&self, _sender: Option<&AnyObject>) {
            self.find_bar().map(|b| guarded(|| b.show(true)));
        }
        #[unsafe(method(findNext:))]
        fn find_next(&self, _sender: Option<&AnyObject>) {
            self.find_bar().map(|b| guarded(|| b.next()));
        }
        #[unsafe(method(findPrevious:))]
        fn find_previous(&self, _sender: Option<&AnyObject>) {
            self.find_bar().map(|b| guarded(|| b.previous()));
        }
    }
);

/// Hidden markers share one visual spot with the character after them, so
/// TextKit's forward step jumps over them and the caret could never stop right
/// before `**` (where typing stays outside the markup). A one-character forward
/// step that ends just past a run of hidden characters stops at the run's start
/// instead; the caret there reveals the markers and the next step is normal.
fn caret_before_hidden(storage: &NSTextStorage, old: NSRange, new: NSRange) -> NSRange {
    if old.length != 0 || new.length != 0 || new.location <= old.location {
        return new;
    }
    let hidden = |i: usize| {
        // SAFETY: a null effective-range pointer is allowed; `i` is in the text.
        unsafe { storage.attribute_atIndex_effectiveRange(hidden_key(), i, std::ptr::null_mut()) }.is_some()
    };
    let mut start = new.location.min(storage.length());
    while start > old.location && hidden(start - 1) {
        start -= 1;
    }
    // Only a one-grapheme step (→), never End, ⌘→, ↓, clicks or pastes, which
    // mean "after the markers".
    let step = storage.string().rangeOfComposedCharacterSequenceAtIndex(old.location);
    if start < new.location && start == step.end() { NSRange::new(start, 0) } else { new }
}

/// Adds `lines` to the pending scroll; a full line either way is one zoom
/// step (its sign) and empties the pending amount.
fn wheel_step(pending: f64, lines: f64) -> (f64, i32) {
    let sum = pending + lines;
    if sum.abs() >= 1.0 { (0.0, sum.signum() as i32) } else { (sum, 0) }
}

/// The editor's least horizontal inset on each side.
const COLUMN_SIDE: f64 = 16.0;

fn range_of(r: NSRange) -> std::ops::Range<usize> {
    r.location..r.end()
}

impl EditorView {
    pub fn storage(&self) -> &NSTextStorage {
        &self.ivars().storage
    }

    pub fn find_bar(&self) -> Option<Retained<FindBar>> {
        self.ivars().find_bar.borrow().load()
    }

    pub fn set_find_bar(&self, bar: &FindBar) {
        *self.ivars().find_bar.borrow_mut() = Weak::new(bar);
    }

    pub fn status(&self) -> Option<Retained<StatusPill>> {
        self.ivars().status.borrow().load()
    }

    pub fn set_status_pill(&self, status: &StatusPill) {
        *self.ivars().status.borrow_mut() = Weak::new(status);
    }

    /// The text container width for `columnWidth` characters of the editor
    /// font ('0' advances), plus the line fragment padding on both sides.
    pub fn column_pts(&self) -> f64 {
        // SAFETY: a plain getter; the editor is built with a container.
        let pad = unsafe { self.textContainer() }.map_or(0.0, |c| c.lineFragmentPadding());
        let font = settings::current_font();
        let attrs = NSDictionary::from_slices(&[unsafe { NSFontAttributeName }], &[&*font as &AnyObject]);
        let zeros = NSString::from_str(&"0".repeat(settings::column_width()));
        // SAFETY: the font attribute's value is a font.
        unsafe { zeros.sizeWithAttributes(Some(&attrs)) }.width + 2.0 * pad
    }

    /// Fixes the container at the column width and centres it in the scroll
    /// view. Narrower only when the window is capped at the screen (huge zoom).
    pub fn column_layout(&self) {
        let Some(scroll) = self.enclosingScrollView() else { return };
        // SAFETY: a plain getter; the editor is built with a container.
        let Some(container) = (unsafe { self.textContainer() }) else { return };
        let avail = scroll.contentSize().width;
        if avail <= 2.0 * COLUMN_SIDE {
            return; // not laid out yet
        }
        let width = self.column_pts().min(avail - 2.0 * COLUMN_SIDE);
        let size = NSSize::new(width, f64::MAX);
        if container.size() != size {
            container.setSize(size);
        }
        let old = self.textContainerInset();
        let inset = NSSize::new(((avail - width) / 2.0).max(COLUMN_SIDE), old.height);
        if old != inset {
            self.setTextContainerInset(inset);
        }
    }

    /// One zoom step in `direction`, or back to 100% for 0. The status shows
    /// even at the ends of the range. Not an edit: no undo, no dirty flag.
    fn zoom_by(&self, direction: i32) {
        guarded(|| {
            let z = if direction == 0 { 1.0 } else { zoom::step(settings::zoom(), direction) };
            settings::set_zoom(z);
            if let Some(status) = self.status() {
                status.flash(&format!("Text size {}%", (z * 100.0).round()));
            }
        });
    }

    pub fn palette(&self) -> Ref<'_, Palette> {
        self.ivars().pal.borrow()
    }

    /// Applies `e` as one undoable user edit (restyled by the storage
    /// delegate), then selects `e.selection`.
    pub fn apply_edit(&self, e: &Edit) {
        let range = NSRange::new(e.range.start, e.range.len());
        let replacement = NSString::from_str(&e.replacement);
        if !self.shouldChangeTextInRange_replacementString(range, Some(&replacement)) {
            return;
        }
        self.storage().replaceCharactersInRange_withString(range, &replacement);
        self.didChangeText();
        self.setSelectedRange(NSRange::new(e.selection.start, e.selection.len()));
    }

    fn emphasis(&self, marker: &str) {
        let text = to_u16(&self.string());
        let sel = range_of(self.selectedRange());
        if let Some(e) = guarded(|| edit::toggle_emphasis(&text, sel, marker)) {
            self.apply_edit(&e);
        }
    }

    /// Nests or lifts the list items under the selection; false when Tab
    /// should type as usual.
    fn indent(&self, outdent: bool) -> bool {
        let text = to_u16(&self.string());
        let sel = range_of(self.selectedRange());
        match guarded(|| list::indent(&text, sel, outdent)).flatten() {
            Some(e) => {
                self.apply_edit(&e);
                true
            }
            None => false,
        }
    }

    /// Colours from the effective appearance, then a full restyle.
    pub fn apply_theme(&self) {
        let names = NSArray::from_slice(&unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] });
        let best = self.effectiveAppearance().bestMatchFromAppearancesWithNames(&names);
        let pal = palette(best.is_some_and(|n| &*n == unsafe { NSAppearanceNameDarkAqua }));
        let font = base_font();
        self.setBackgroundColor(&pal.bg);
        self.setInsertionPointColor(Some(&pal.text));
        self.setFont(Some(&font));
        self.setTextColor(Some(&pal.text));
        let selected = NSDictionary::from_slices(
            &unsafe { [NSBackgroundColorAttributeName, NSForegroundColorAttributeName] },
            &[pal.selection.as_ref(), pal.text.as_ref()],
        );
        // SAFETY: both values are colours, as the keys require.
        unsafe { self.setSelectedTextAttributes(&selected) };
        if let Ok(mut styler) = self.ivars().styler.try_borrow_mut() {
            let caret = self.selectedRange().location;
            guarded(|| styler.restyle_all(self.storage(), &font, &pal, caret));
        }
        *self.ivars().pal.borrow_mut() = pal;
        *self.ivars().font.borrow_mut() = font;
        if let Some(bar) = self.find_bar() {
            guarded(|| bar.paint());
        }
    }
}

/// Builds the editor on an explicit TextKit 1 chain
/// (NSTextStorage -> NSLayoutManager -> NSTextContainer).
pub fn make_editor(frame: NSRect) -> Retained<EditorView> {
    let mtm = MainThreadMarker::new().expect("editor is built on the main thread");
    let storage = NSTextStorage::new();
    let layout = NSLayoutManager::new();
    storage.addLayoutManager(&layout);
    let container = NSTextContainer::initWithSize(
        NSTextContainer::alloc(),
        NSSize::new(frame.size.width, f64::MAX),
    );
    // The width is the column's, set by column_layout.
    container.setWidthTracksTextView(false);
    layout.addTextContainer(&container);

    let this = EditorView::alloc(mtm).set_ivars(Ivars {
        storage,
        styler: RefCell::new(Styler::default()),
        caret: Cell::new(0),
        pal: RefCell::new(palette(false)),
        find_bar: RefCell::default(),
        status: RefCell::default(),
        font: RefCell::new(base_font()),
        wheel: Cell::new(0.0),
        observers: RefCell::default(),
    });
    // SAFETY: designated initializer of NSTextView with matching argument types.
    let editor: Retained<EditorView> =
        unsafe { msg_send![super(this), initWithFrame: frame, textContainer: &*container] };

    editor.setRichText(false);
    editor.setAllowsUndo(true);
    editor.setAutomaticQuoteSubstitutionEnabled(false);
    editor.setAutomaticDashSubstitutionEnabled(false);
    editor.setAutomaticTextReplacementEnabled(false);
    editor.setContinuousSpellCheckingEnabled(true);
    editor.setMinSize(NSSize::new(0.0, frame.size.height));
    editor.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
    editor.setVerticallyResizable(true);
    editor.setHorizontallyResizable(false);
    editor.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    // Delegates are weak references in AppKit: no retain cycle.
    let me = ProtocolObject::from_ref(&*editor);
    editor.storage().setDelegate(Some(me));
    layout.setDelegate(Some(ProtocolObject::from_ref(&*editor)));
    editor.setDelegate(Some(ProtocolObject::from_ref(&*editor)));
    editor.apply_theme();
    let weak = Weak::new(&*editor);
    // Settings are only changed on the main thread, where the editor lives.
    let restyle = observe(settings::CHANGED, None, move || {
        if let Some(e) = weak.load() {
            e.apply_theme();
            e.column_layout();
        }
    });
    editor.ivars().observers.borrow_mut().push(restyle);
    editor
}

/// The editor as the document view of a vertically scrolling scroll view.
pub fn editor_in_scroll(frame: NSRect) -> (Retained<NSScrollView>, Retained<EditorView>) {
    let mtm = MainThreadMarker::new().expect("editor is built on the main thread");
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let editor = make_editor(NSRect::new(NSPoint::new(0.0, 0.0), scroll.contentSize()));
    scroll.setDocumentView(Some(&editor));
    let clip = scroll.contentView();
    clip.setPostsFrameChangedNotifications(true);
    let weak = Weak::new(&*editor);
    // SAFETY: NSViewFrameDidChangeNotification is a constant string.
    let name = unsafe { objc2_app_kit::NSViewFrameDidChangeNotification }.to_string();
    let resized = observe(&name, Some(&clip), move || {
        if let Some(e) = weak.load() {
            e.column_layout();
        }
    });
    editor.ivars().observers.borrow_mut().push(resized);
    editor.column_layout();
    (scroll, editor)
}

#[cfg(test)]
mod tests {
    use super::wheel_step;

    #[test]
    fn wheel_steps_once_per_line() {
        assert_eq!(wheel_step(0.0, 0.4), (0.4, 0));
        assert_eq!(wheel_step(0.4, 0.7), (0.0, 1));
        assert_eq!(wheel_step(0.0, -3.0), (0.0, -1)); // one step per event, not per line
        assert_eq!(wheel_step(0.5, -0.5), (0.0, 0)); // reversing cancels
    }
}
