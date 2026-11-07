//! VoiceOver opens a status menu by pressing its button (AXPress, VO-Space), which sends the
//! button's action. tray-icon opens Yard's menu from mouse events in a view over the button and
//! gives the button no target, so VoiceOver found "Yard, status menu" and could not open it (found
//! with VoiceOver on 2026-11-07). The button gets a target whose action calls `press`; the menu
//! bar opens the menu the way a click does (`TrayIcon::show_menu`).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSButton, NSView};
use std::cell::{Cell, RefCell};

thread_local! {
    static ON_PRESS: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
    /// Set while the menu is being opened: `show_menu` clicks the button itself, which must not
    /// ask for the menu again.
    static OPENING: Cell<bool> = const { Cell::new(false) };
    static TARGET: RefCell<Option<Retained<PressTarget>>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YardStatusPressTarget"]
    struct PressTarget;

    impl PressTarget {
        #[unsafe(method(press:))]
        fn press(&self, _sender: Option<&AnyObject>) {
            if OPENING.get() {
                return;
            }
            ON_PRESS.with(|f| {
                if let Some(f) = f.borrow().as_ref() {
                    f();
                }
            });
        }
    }
);

/// Runs `open` while the menu opens, so the button's own click inside it is ignored.
pub fn opening(open: impl FnOnce()) {
    OPENING.set(true);
    open();
    OPENING.set(false);
}

fn class_name(view: &NSView) -> String {
    let class = (**view).class();
    class.name().to_string_lossy().into_owned()
}

fn find_button(view: &NSView) -> Option<Retained<NSView>> {
    if class_name(view).contains("StatusBarButton") {
        return Some(view.retain());
    }
    view.subviews().iter().find_map(|v| find_button(&v))
}

/// Gives the app's status bar button a target, so an accessibility press calls `on_press`.
/// Returns false when no status bar button was found.
pub fn install(on_press: impl Fn() + 'static) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let app = NSApplication::sharedApplication(mtm);
    let button = app.windows().iter().find_map(|w| w.contentView().and_then(|v| find_button(&v)));
    let Some(button) = button else { return false };
    let target: Retained<PressTarget> = unsafe { msg_send![PressTarget::alloc(mtm), init] };
    // SAFETY: the view is an NSStatusBarButton, an NSButton.
    let button: &NSButton = unsafe { &*(Retained::as_ptr(&button) as *const NSButton) };
    unsafe {
        button.setTarget(Some(&target));
        button.setAction(Some(sel!(press:)));
    }
    ON_PRESS.with(|f| *f.borrow_mut() = Some(Box::new(on_press)));
    TARGET.with(|t| *t.borrow_mut() = Some(target));
    true
}
