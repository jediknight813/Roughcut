//! Native app identity for standalone terminal launches as well as app bundles.
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

pub fn set_application_icon() {
    let main_thread = MainThreadMarker::new().expect("app startup runs on the main thread");
    let data = NSData::with_bytes(include_bytes!("../assets/brand/roughcut-icon.png"));
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        eprintln!("roughcut: could not decode the application icon");
        return;
    };
    let app = NSApplication::sharedApplication(main_thread);
    // SAFETY: the setter requires a non-null image; AppKit retains this decoded image.
    unsafe {
        app.setApplicationIconImage(Some(&icon));
    }
}
