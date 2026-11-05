//! The status item and its menu (M6 design §8.1): a native `NSStatusItem` and `NSMenu` through
//! tray-icon and muda, with GPUI only for windows. Task 7 places it with one item; Task 13 builds
//! the real menu from the app's state.

use futures::StreamExt;
use futures::channel::mpsc;
use gpui::App;
use muda::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub const QUIT: &str = "quit";

/// Keeps the status item alive; dropping it removes the item from the menu bar.
pub struct MenuBar {
    _tray: TrayIcon,
}

impl gpui::Global for MenuBar {}

/// Puts the status item in the menu bar and forwards menu clicks into GPUI.
pub fn install(cx: &mut App) -> Result<(), String> {
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(QUIT, "Quit Yard.app", true, None)).map_err(|e| e.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon_templated(template_icon())
        .with_tooltip("Yard")
        .build()
        .map_err(|e| e.to_string())?;
    // muda calls the handler on the main thread, outside GPUI; a channel brings the click in.
    let (tx, mut rx) = mpsc::unbounded::<String>();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = tx.unbounded_send(event.id.0);
    }));
    cx.spawn(async move |cx| {
        while let Some(id) = rx.next().await {
            if id == QUIT {
                cx.update(|cx| cx.quit());
            }
        }
    })
    .detach();
    cx.set_global(MenuBar { _tray: tray });
    Ok(())
}

/// An 18×18 template image (black on clear; macOS tints it): a filled circle with a notch, a
/// placeholder until the per-health icons of Task 13.
fn template_icon() -> Icon {
    const N: u32 = 36;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 17.5, y as f32 - 17.5);
            let r = (dx * dx + dy * dy).sqrt();
            let on = r < 15.0 && !(r < 9.0 && dy < 0.0 && dx.abs() < 3.0);
            rgba.extend_from_slice(if on { &[0, 0, 0, 255] } else { &[0, 0, 0, 0] });
        }
    }
    Icon::from_rgba(rgba, N, N).expect("a 36×36 RGBA icon")
}
