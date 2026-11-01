pub mod app_delegate;
pub mod content;
pub mod document;
pub mod editor;
pub mod export;
pub mod find_bar;
pub mod glass;
pub mod menu;
pub mod preview;
pub mod settings;
pub mod status;
pub mod styler;
pub mod theme;
pub mod titlebar;
pub mod toolbar;
pub mod util;

use objc2::ClassType;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

/// Bootstraps and runs the app; never returns.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    // NSDocumentController looks the class up by name (Info.plist), so register it first.
    let _ = document::PrasastiDocument::class();
    let app = NSApplication::sharedApplication(mtm);
    let delegate = app_delegate::AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    menu::install(&app, mtm);
    titlebar::apply_appearance(mtm);
    app.run();
}
