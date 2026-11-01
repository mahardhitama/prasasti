use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackgroundColorAttributeName, NSButton, NSControl, NSControlTextEditingDelegate, NSEventModifierFlags,
    NSLayoutConstraintOrientation, NSResponder, NSSearchField, NSSearchFieldDelegate, NSStackView,
    NSTextField, NSTextFieldDelegate, NSTextView, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSEdgeInsets, NSNotification, NSObject, NSObjectProtocol, NSRange, NSRect, NSString,
};
use prasasti_core::edit::Edit;
use prasasti_core::search::find_matches;

use crate::editor::EditorView;
use crate::util::{guarded, to_u16};

pub struct Ivars {
    editor: Retained<EditorView>,
    search: Retained<NSSearchField>,
    replace: Retained<NSTextField>,
    /// The replace field and its buttons, hidden in find-only mode.
    replace_views: Vec<Retained<NSView>>,
    /// Start offsets of the matches; each is as long as the query.
    matches: RefCell<Vec<usize>>,
}

define_class!(
    /// The find / replace row above the editor. Matches are painted as
    /// layout-manager temporary attributes, so they never touch the document.
    // SAFETY: NSStackView has no subclassing requirements we violate; no Drop.
    #[unsafe(super(NSStackView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct FindBar;

    unsafe impl NSObjectProtocol for FindBar {}

    unsafe impl NSControlTextEditingDelegate for FindBar {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, note: &NSNotification) {
            let from_search = note.object().is_some_and(|o| std::ptr::eq(&*o, self.ivars().search.as_ref() as &AnyObject));
            if from_search {
                guarded(|| self.search(self.ivars().editor.selectedRange().location));
            }
        }

        /// Return: next, Shift-Return: previous, Esc: close, in either field.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        unsafe fn do_command(&self, _control: &NSControl, tv: &NSTextView, cmd: Sel) -> bool {
            guarded(|| {
                if cmd == sel!(insertNewline:) {
                    // Shift-Return reaches the field editor as insertNewline: too.
                    let shift = tv.window().and_then(|w| w.currentEvent()).is_some_and(|e| {
                        e.modifierFlags().contains(NSEventModifierFlags::Shift)
                    });
                    if shift { self.previous() } else { self.next() }
                } else if cmd == sel!(cancelOperation:) {
                    self.hide();
                } else {
                    return false;
                }
                true
            })
            .unwrap_or(false)
        }
    }

    unsafe impl NSTextFieldDelegate for FindBar {}
    unsafe impl NSSearchFieldDelegate for FindBar {}

    /// The menu's find actions while a bar field has focus.
    impl FindBar {
        #[unsafe(method(showFindBar:))]
        fn show_find_bar(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.show(false));
        }
        #[unsafe(method(showFindReplaceBar:))]
        fn show_find_replace_bar(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.show(true));
        }
        #[unsafe(method(findNext:))]
        fn find_next(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.next());
        }
        #[unsafe(method(findPrevious:))]
        fn find_previous(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.previous());
        }
        #[unsafe(method(replaceOne:))]
        fn replace_one_action(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.replace_one());
        }
        #[unsafe(method(replaceAll:))]
        fn replace_all_action(&self, _sender: Option<&AnyObject>) {
            guarded(|| self.replace_all());
        }
        /// Deferred from edits: temporary attributes can't change mid-edit.
        #[unsafe(method(repaint))]
        fn repaint_action(&self) {
            guarded(|| self.paint());
        }
    }
);

impl FindBar {
    /// A hidden bar for `editor`, which it registers with.
    pub fn new(mtm: MainThreadMarker, editor: &EditorView) -> Retained<Self> {
        let search = NSSearchField::new(mtm);
        let replace = NSTextField::textFieldWithString(&NSString::new(), mtm);
        replace.setPlaceholderString(Some(&NSString::from_str("Replace")));
        // SAFETY: both fields live as long as the bar, which holds them.
        unsafe {
            search.setNextKeyView(Some(&replace));
            replace.setNextKeyView(Some(&search));
        }
        // SAFETY: each action is a `-(void)x:(id)sender` method of the bar,
        // which becomes the target below.
        let button = |title: &str, action: Sel| unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str(title), None, Some(action), mtm)
        };
        let buttons = [
            button("Previous", sel!(findPrevious:)),
            button("Next", sel!(findNext:)),
            button("Replace", sel!(replaceOne:)),
            button("Replace All", sel!(replaceAll:)),
        ];
        let view = |v: &NSView| v.retain();
        let replace_views = vec![view(&replace), view(&buttons[2]), view(&buttons[3])];
        let this = Self::alloc(mtm).set_ivars(Ivars {
            editor: editor.retain(),
            search,
            replace,
            replace_views,
            matches: RefCell::new(vec![]),
        });
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        let ivars = this.ivars();
        for b in &buttons {
            // SAFETY: the bar outlives its own buttons.
            unsafe { b.setTarget(Some(&this)) };
        }
        let [prev, next, one, all] = &buttons;
        for v in [&*ivars.search as &NSView, prev, next, &ivars.replace, one, all] {
            this.addArrangedSubview(v);
        }
        this.setEdgeInsets(NSEdgeInsets { top: 6.0, left: 8.0, bottom: 6.0, right: 8.0 });
        // Stay one row tall; the editor below takes the height.
        this.setHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Vertical);
        // SAFETY: delegates are weak; the bar outlives its own fields.
        unsafe {
            ivars.search.setDelegate(Some(ProtocolObject::from_ref(&*this)));
            ivars.replace.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        }
        this.setHidden(true);
        editor.set_find_bar(&this);
        this
    }

    /// Shows the bar, focuses the search field and searches for what it holds.
    pub fn show(&self, replace: bool) {
        for v in &self.ivars().replace_views {
            v.setHidden(!replace);
        }
        self.setHidden(false);
        if let Some(w) = self.window() {
            w.makeFirstResponder(Some(&self.ivars().search));
        }
        self.search(self.ivars().editor.selectedRange().location);
    }

    /// Hides the bar, clears the highlights and hands focus back to the editor.
    pub fn hide(&self) {
        self.setHidden(true);
        self.ivars().matches.borrow_mut().clear();
        self.paint();
        let editor = &self.ivars().editor;
        if let Some(w) = self.window() {
            w.makeFirstResponder(Some(editor));
        }
    }

    pub fn next(&self) {
        let sel = self.ivars().editor.selectedRange();
        self.select_from(sel.end());
    }

    pub fn previous(&self) {
        let start = self.ivars().editor.selectedRange().location;
        let m = self.ivars().matches.borrow();
        let Some(&at) = m.iter().rev().find(|&&s| s < start).or(m.last()) else { return };
        drop(m);
        self.select(at);
    }

    /// Replaces the selected match, if the selection is one, then selects the next.
    pub fn replace_one(&self) {
        let editor = &self.ivars().editor;
        let sel = editor.selectedRange();
        let len = self.query().len();
        let selected = len > 0 && sel.length == len && self.ivars().matches.borrow().contains(&sel.location);
        if !selected {
            return self.select_from(sel.location);
        }
        let replacement = self.ivars().replace.stringValue().to_string();
        let end = sel.location + replacement.encode_utf16().count();
        editor.apply_edit(&Edit { range: sel.location..sel.end(), replacement, selection: end..end });
        self.select_from(end);
    }

    /// Replaces every match as one edit, so one undo step.
    pub fn replace_all(&self) {
        let len = self.query().len();
        let m = self.matches();
        let (Some(&first), Some(&last)) = (m.first(), m.last()) else { return };
        let text = to_u16(&self.ivars().editor.string());
        let with = to_u16(&self.ivars().replace.stringValue());
        let mut out = vec![];
        let mut at = first;
        for &s in &m {
            out.extend_from_slice(&text[at..s]);
            out.extend_from_slice(&with);
            at = s + len;
        }
        // Matches never split a surrogate pair; refuse rather than corrupt if one did.
        let Ok(replacement) = String::from_utf16(&out) else { return };
        let end = first + out.len();
        self.ivars().editor.apply_edit(&Edit { range: first..last + len, replacement, selection: end..end });
    }

    pub fn matches(&self) -> Vec<usize> {
        self.ivars().matches.borrow().clone()
    }

    /// Called by the editor after each edit: recompute, never touch the selection.
    pub fn text_changed(&self) {
        if self.isHidden() {
            return;
        }
        self.recompute();
        // SAFETY: `repaint` takes no argument; a pending one is replaced.
        unsafe {
            let () = msg_send![NSObject::class(), cancelPreviousPerformRequestsWithTarget: self, selector: sel!(repaint), object: None::<&AnyObject>];
            let () = msg_send![self, performSelector: sel!(repaint), withObject: None::<&AnyObject>, afterDelay: 0.0f64];
        }
    }

    fn query(&self) -> Vec<u16> {
        to_u16(&self.ivars().search.stringValue())
    }

    fn recompute(&self) {
        let text = to_u16(&self.ivars().editor.string());
        *self.ivars().matches.borrow_mut() = find_matches(&text, &self.query());
    }

    /// New query: matches again, selecting the first at or after `from`.
    fn search(&self, from: usize) {
        self.recompute();
        self.select_from(from);
    }

    /// Selects the first match starting at or after `from`, wrapping; repaints.
    fn select_from(&self, from: usize) {
        let m = self.ivars().matches.borrow();
        let at = m.iter().find(|&&s| s >= from).or(m.first()).copied();
        drop(m);
        match at {
            Some(at) => self.select(at),
            None => self.paint(),
        }
    }

    fn select(&self, at: usize) {
        let r = NSRange::new(at, self.query().len());
        let editor = &self.ivars().editor;
        editor.setSelectedRange(r);
        editor.scrollRangeToVisible(r);
        self.paint();
    }

    /// Search colour on every match, current colour on the one the editor selects.
    pub fn paint(&self) {
        let editor = &self.ivars().editor;
        // SAFETY: a plain getter; the editor always has a layout manager.
        let Some(lm) = (unsafe { editor.layoutManager() }) else { return };
        let key = unsafe { NSBackgroundColorAttributeName };
        let len = editor.string().length();
        lm.removeTemporaryAttribute_forCharacterRange(key, NSRange::new(0, len));
        let qlen = self.query().len();
        let sel = editor.selectedRange();
        let pal = editor.palette();
        for &s in self.ivars().matches.borrow().iter().filter(|&&s| s + qlen <= len) {
            let r = NSRange::new(s, qlen);
            let colour = if r == sel { &pal.current } else { &pal.search };
            // SAFETY: the value is a colour, as the key requires; `r` is in the text.
            unsafe { lm.addTemporaryAttribute_value_forCharacterRange(key, colour, r) };
        }
    }
}
