//! Colours and spacing for light and dark, following the window's appearance.

use gpui::{Hsla, Window, WindowAppearance, hsla, rgb};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
}

impl Theme {
    pub fn of(window: &Window) -> Theme {
        match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::light(),
        }
    }

    pub fn light() -> Theme {
        Theme {
            background: rgb(0xf6f6f4).into(),
            surface: rgb(0xffffff).into(),
            text: rgb(0x1d1d1f).into(),
            muted: rgb(0x6e6e73).into(),
            border: rgb(0xd8d8d4).into(),
            accent: rgb(0x1f7a4d).into(),
            on_accent: rgb(0xffffff).into(),
            danger: rgb(0xc62828).into(),
            warning: rgb(0xa86400).into(),
        }
    }

    pub fn dark() -> Theme {
        Theme {
            background: rgb(0x1c1c1e).into(),
            surface: rgb(0x2a2a2d).into(),
            text: rgb(0xf2f2f2).into(),
            muted: rgb(0x9a9aa0).into(),
            border: rgb(0x3d3d41).into(),
            accent: rgb(0x3fb37a).into(),
            on_accent: rgb(0x0b1f14).into(),
            danger: rgb(0xff6b6b).into(),
            warning: rgb(0xf0b44c).into(),
        }
    }

    /// A translucent wash of `color` for a banner or a selected row.
    pub fn wash(color: Hsla) -> Hsla {
        hsla(color.h, color.s, color.l, 0.14)
    }
}
