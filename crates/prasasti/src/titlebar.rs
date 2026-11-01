//! The document window's native toolbar: the window title centred (the unified
//! style puts the native title at the leading edge) and, at the trailing edge,
//! a button that cycles the app's appearance System → Light → Dark.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSButton, NSFont,
    NSImage, NSImageView, NSLineBreakMode, NSTextField, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSView, NSWindow, NSWindowToolbarStyle,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSKeyValueObservingOptions, NSObject, NSObjectNSKeyValueObserverRegistration,
    NSObjectProtocol, NSSet, NSString, ns_string,
};

use crate::settings;
use crate::util::{Observer, guarded, observe};

pub const TITLE_ITEM: &str = "PrasastiTitle";
const APPEARANCE_ITEM: &str = "PrasastiAppearance";

pub struct Ivars {
    window: Weak<NSWindow>,
    label: Retained<NSTextField>,
    button: Retained<NSButton>,
    icon: Retained<NSImageView>,
    /// The mode the button shows ("" before the first refresh).
    shown: Cell<&'static str>,
    /// Registered for KVO on the window's title until it closes.
    observing: Cell<bool>,
    observers: RefCell<Vec<Observer>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct Titlebar;

    unsafe impl NSObjectProtocol for Titlebar {}

    unsafe impl NSToolbarDelegate for Titlebar {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(&self, _toolbar: &NSToolbar, id: &NSString, _flag: bool) -> Option<Retained<NSToolbarItem>> {
            guarded(|| self.make_item(id)).flatten()
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            guarded(ids).unwrap_or_default()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            guarded(ids).unwrap_or_default()
        }
    }

    impl Titlebar {
        #[unsafe(method(cycleAppearance:))]
        fn cycle_appearance(&self, _sender: Option<&AnyObject>) {
            guarded(|| {
                settings::set_appearance(match settings::appearance() {
                    "system" => "light",
                    "light" => "dark",
                    _ => "system",
                });
                apply_appearance(self.mtm());
            });
        }

        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(&self, _key: Option<&NSString>, _object: Option<&AnyObject>, _change: Option<&AnyObject>, _context: *mut c_void) {
            guarded(|| self.sync_title());
        }
    }
);

fn ids() -> Retained<NSArray<NSString>> {
    // SAFETY: a constant string.
    let flexible = unsafe { NSToolbarFlexibleSpaceItemIdentifier };
    NSArray::from_slice(&[ns_string!(TITLE_ITEM), flexible, ns_string!(APPEARANCE_ITEM)])
}

/// Sets NSApp's appearance from the setting: None follows the system.
pub fn apply_appearance(mtm: MainThreadMarker) {
    // SAFETY: plain extern statics.
    let name = match settings::appearance() {
        "light" => Some(unsafe { NSAppearanceNameAqua }),
        "dark" => Some(unsafe { NSAppearanceNameDarkAqua }),
        _ => None,
    };
    NSApplication::sharedApplication(mtm).setAppearance(name.and_then(NSAppearance::appearanceNamed).as_deref());
}

/// The appearance button in `window`'s toolbar.
pub fn appearance_button(window: &NSWindow) -> Option<Retained<NSButton>> {
    let item = window.toolbar()?.items().iter().find(|i| &*i.itemIdentifier() == ns_string!(APPEARANCE_ITEM))?;
    item.view()?.downcast().ok()
}

impl Titlebar {
    /// Gives `window` its toolbar; the caller keeps the returned delegate while the window lives.
    pub fn install(window: &NSWindow) -> Retained<Self> {
        let mtm = window.mtm();
        let label = NSTextField::labelWithString(&window.title(), mtm);
        label.setFont(Some(&NSFont::titleBarFontOfSize(0.0)));
        label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        // SAFETY: no target yet (set below, to the delegate); an action selector.
        let button = unsafe { NSButton::buttonWithTitle_target_action(ns_string!(""), None, Some(sel!(cycleAppearance:)), mtm) };
        button.setBordered(false);
        let icon = NSImageView::new(mtm);
        icon.setAccessibilityElement(false);
        icon.setTranslatesAutoresizingMaskIntoConstraints(false);
        button.addSubview(&icon);
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        button.widthAnchor().constraintEqualToConstant(32.0).setActive(true);
        button.heightAnchor().constraintEqualToConstant(28.0).setActive(true);
        icon.centerXAnchor().constraintEqualToAnchor(&button.centerXAnchor()).setActive(true);
        icon.centerYAnchor().constraintEqualToAnchor(&button.centerYAnchor()).setActive(true);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            window: Weak::new(window),
            label,
            button,
            icon,
            shown: Cell::new(""),
            observing: Cell::new(false),
            observers: RefCell::default(),
        });
        // SAFETY: NSObject's init.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        let iv = this.ivars();
        // SAFETY: NSControl holds its target weakly; the window's owner keeps `this`.
        unsafe { iv.button.setTarget(Some(&this)) };
        this.refresh();

        let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), ns_string!("PrasastiToolbar"));
        toolbar.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*this)));
        toolbar.setAllowsUserCustomization(false);
        toolbar.setAutosavesConfiguration(false);
        toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        toolbar.setCenteredItemIdentifiers(&NSSet::from_slice(&[ns_string!(TITLE_ITEM)]));
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        window.setToolbar(Some(&toolbar));

        // SAFETY: `this` implements observeValueForKeyPath:…; removed when the window closes.
        unsafe {
            window.addObserver_forKeyPath_options_context(&this, ns_string!("title"), NSKeyValueObservingOptions::Initial, std::ptr::null_mut())
        };
        iv.observing.set(true);
        let weak = Weak::new(&*this);
        let changed = observe(settings::CHANGED, None, move || {
            if let Some(t) = weak.load() {
                t.refresh();
            }
        });
        let weak = Weak::new(&*this);
        let sender: &AnyObject = window;
        let closing = observe("NSWindowWillCloseNotification", Some(sender), move || {
            if let Some(t) = weak.load() {
                t.stop_observing();
            }
        });
        *iv.observers.borrow_mut() = vec![changed, closing];
        this
    }

    fn make_item(&self, id: &NSString) -> Option<Retained<NSToolbarItem>> {
        let iv = self.ivars();
        let view: &NSView = if id == ns_string!(TITLE_ITEM) {
            &iv.label
        } else if id == ns_string!(APPEARANCE_ITEM) {
            &iv.button
        } else {
            return None;
        };
        let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(self.mtm()), id);
        item.setView(Some(view));
        if id == ns_string!(APPEARANCE_ITEM) {
            item.setLabel(ns_string!("Appearance"));
        }
        Some(item)
    }

    fn sync_title(&self) {
        if let Some(w) = self.ivars().window.load() {
            self.ivars().label.setStringValue(&w.title());
        }
    }

    fn stop_observing(&self) {
        let iv = self.ivars();
        if let Some(w) = iv.window.load().filter(|_| iv.observing.replace(false)) {
            // SAFETY: registered in `install` with this observer and key path.
            unsafe { w.removeObserver_forKeyPath(self, ns_string!("title")) };
        }
    }

    /// Shows the current setting's symbol, swapping it with the replace transition once on screen.
    fn refresh(&self) {
        let iv = self.ivars();
        let mode = settings::appearance();
        let previous = iv.shown.replace(mode);
        if previous == mode {
            return;
        }
        let (symbol, name) = match mode {
            "light" => ("sun.max", "Light"),
            "dark" => ("moon", "Dark"),
            _ => ("circle.lefthalf.filled", "System"),
        };
        let label = NSString::from_str(&format!("Appearance: {name}"));
        iv.button.setToolTip(Some(&label));
        iv.button.setAccessibilityLabel(Some(&label));
        let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(symbol), None) else {
            return;
        };
        // objc2-app-kit 0.3.2 has no Symbols bindings; the class ships with macOS 14+.
        match AnyClass::get(c"NSSymbolReplaceContentTransition").filter(|_| !previous.is_empty()) {
            // SAFETY: +[NSSymbolReplaceContentTransition transition] returns an
            // NSSymbolContentTransition, which -[NSImageView setSymbolImage:withContentTransition:] takes.
            Some(cls) => unsafe {
                let transition: Retained<AnyObject> = msg_send![cls, transition];
                let () = msg_send![&*iv.icon, setSymbolImage: &*image, withContentTransition: &*transition];
            },
            None => iv.icon.setImage(Some(&image)),
        }
    }
}
