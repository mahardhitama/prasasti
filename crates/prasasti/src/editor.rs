use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSLayoutManager, NSResponder, NSText, NSTextContainer,
    NSTextStorage, NSTextView, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSRect, NSSize};

define_class!(
    /// The plain-text Markdown editor. Action stubs exist now so their menu
    /// items validate; later tasks fill them in.
    // SAFETY: NSTextView has no subclassing requirements we violate; no Drop.
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Retained<NSTextStorage>]
    pub struct EditorView;

    impl EditorView {
        #[unsafe(method(toggleBold:))]
        fn toggle_bold(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(toggleItalic:))]
        fn toggle_italic(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(toggleStrikethrough:))]
        fn toggle_strikethrough(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(insertLink:))]
        fn insert_link(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(copyAsHTML:))]
        fn copy_as_html(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(zoomIn:))]
        fn zoom_in(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(zoomOut:))]
        fn zoom_out(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(resetZoom:))]
        fn reset_zoom(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(showFindBar:))]
        fn show_find_bar(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(showFindReplaceBar:))]
        fn show_find_replace_bar(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(findNext:))]
        fn find_next(&self, _sender: Option<&AnyObject>) {}
        #[unsafe(method(findPrevious:))]
        fn find_previous(&self, _sender: Option<&AnyObject>) {}
    }
);

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
    container.setWidthTracksTextView(true);
    layout.addTextContainer(&container);

    // The view does not retain its storage (the storage owns the layout manager,
    // which owns the container), so the view keeps it alive as its ivar.
    let this = EditorView::alloc(mtm).set_ivars(storage);
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
    editor
}
