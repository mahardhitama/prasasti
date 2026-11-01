use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSFontManager, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};

const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;
const OPT_CMD: NSEventModifierFlags = NSEventModifierFlags::Option.union(CMD);
const CTRL_CMD: NSEventModifierFlags = NSEventModifierFlags::Control.union(CMD);

/// A menu entry: `None` is a separator. An upper-case key adds Shift.
type Entry = Option<(&'static str, Sel, &'static str, NSEventModifierFlags)>;

fn item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str) -> Retained<NSMenuItem> {
    // SAFETY: every action is an Objective-C action selector (`-(void)x:(id)sender`).
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

fn menu(mtm: MainThreadMarker, title: &str, entries: &[Entry]) -> Retained<NSMenu> {
    let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    for e in entries {
        match e {
            None => m.addItem(&NSMenuItem::separatorItem(mtm)),
            Some((title, action, key, mods)) => {
                let i = item(mtm, title, Some(*action), key);
                i.setKeyEquivalentModifierMask(*mods);
                m.addItem(&i);
            }
        }
    }
    m
}

fn add_submenu(mtm: MainThreadMarker, parent: &NSMenu, sub: &NSMenu) {
    let i = item(mtm, &sub.title().to_string(), None, "");
    i.setSubmenu(Some(sub));
    parent.addItem(&i);
}

/// Builds the main menu and installs it on `app`.
pub fn install(app: &NSApplication, mtm: MainThreadMarker) {
    let main = NSMenu::new(mtm);
    let s = |t, a, k, m| Some((t, a, k, m));

    let app_menu = menu(mtm, "Praśasti", &[
        s("About Praśasti", sel!(orderFrontStandardAboutPanel:), "", CMD),
        None,
        None, // Services goes between these separators, below.
        s("Hide Praśasti", sel!(hide:), "h", CMD),
        s("Hide Others", sel!(hideOtherApplications:), "h", OPT_CMD),
        s("Show All", sel!(unhideAllApplications:), "", CMD),
        None,
        s("Quit Praśasti", sel!(terminate:), "q", CMD),
    ]);
    let file = menu(mtm, "File", &[
        s("New", sel!(newDocument:), "n", CMD),
        s("Open…", sel!(openDocument:), "o", CMD),
        None,
        s("Close", sel!(performClose:), "w", CMD),
        s("Save", sel!(saveDocument:), "s", CMD),
        s("Save As…", sel!(saveDocumentAs:), "S", CMD),
        s("Revert to Saved", sel!(revertDocumentToSaved:), "", CMD),
        None,
        s("Page Setup…", sel!(runPageLayout:), "", CMD),
        s("Print…", sel!(printDocument:), "p", CMD),
    ]);
    let edit = menu(mtm, "Edit", &[
        s("Undo", sel!(undo:), "z", CMD),
        s("Redo", sel!(redo:), "Z", CMD),
        None,
        s("Cut", sel!(cut:), "x", CMD),
        s("Copy", sel!(copy:), "c", CMD),
        s("Paste", sel!(paste:), "v", CMD),
        s("Paste and Match Style", sel!(pasteAsPlainText:), "V", OPT_CMD),
        s("Select All", sel!(selectAll:), "a", CMD),
        None,
        s("Copy as HTML", sel!(copyAsHTML:), "C", CMD),
        None,
    ]);
    // Our own find bar (FindBar), not AppKit's performFindPanelAction:.
    let find = menu(mtm, "Find", &[
        s("Find…", sel!(showFindBar:), "f", CMD),
        s("Find and Replace…", sel!(showFindReplaceBar:), "f", OPT_CMD),
        s("Find Next", sel!(findNext:), "g", CMD),
        s("Find Previous", sel!(findPrevious:), "G", CMD),
    ]);
    add_submenu(mtm, &edit, &find);
    let format = menu(mtm, "Format", &[
        s("Bold", sel!(toggleBold:), "b", CMD),
        s("Italic", sel!(toggleItalic:), "i", CMD),
        s("Strikethrough", sel!(toggleStrikethrough:), "X", CMD),
        s("Link", sel!(insertLink:), "k", CMD),
        None,
        s("Show Fonts", sel!(orderFrontFontPanel:), "t", CMD),
    ]);
    // No responder implements this; the font manager does, as in AppKit's own menus.
    if let Some(fonts) = format.itemWithTitle(&NSString::from_str("Show Fonts")) {
        // SAFETY: the shared font manager lives for the whole app.
        unsafe { fonts.setTarget(Some(&NSFontManager::sharedFontManager(mtm))) };
    }
    let view = menu(mtm, "View", &[
        s("Zoom In", sel!(zoomIn:), "=", CMD),
        s("Zoom Out", sel!(zoomOut:), "-", CMD),
        s("Actual Size", sel!(resetZoom:), "0", CMD),
        None,
        s("Enter Full Screen", sel!(toggleFullScreen:), "f", CTRL_CMD),
    ]);
    // ⌘+ (Shift-= on US layouts) zooms in too, from a hidden item.
    let plus = item(mtm, "Zoom In", Some(sel!(zoomIn:)), "+");
    plus.setHidden(true);
    plus.setAllowsKeyEquivalentWhenHidden(true);
    view.insertItem_atIndex(&plus, 1);
    let window = menu(mtm, "Window", &[
        s("Minimize", sel!(performMiniaturize:), "m", CMD),
        s("Zoom", sel!(performZoom:), "", CMD),
        None,
        s("Bring All to Front", sel!(arrangeInFront:), "", CMD),
    ]);

    // AppKit fills the Services submenu once it is registered.
    let services = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Services"));
    let services_item = item(mtm, "Services", None, "");
    services_item.setSubmenu(Some(&services));
    app_menu.insertItem_atIndex(&services_item, 2);
    app.setServicesMenu(Some(&services));

    for m in [&app_menu, &file, &edit, &format, &view, &window] {
        add_submenu(mtm, &main, m);
    }
    app.setMainMenu(Some(&main));
    app.setWindowsMenu(Some(&window));
}
