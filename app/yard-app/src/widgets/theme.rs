//! Colours, spacing, sizes and type for light and dark, following the window's appearance. The
//! values are the visual design's tokens (`2026-11-09-yard-app-visual-design.md` §3).

#[cfg(test)]
use gpui::Rgba;
use gpui::{FontWeight, Hsla, Styled, Window, WindowAppearance, hsla, px, rgb};

/// How much of `sidebar_bg` covers the blurred desktop under the sidebar.
const SIDEBAR_ALPHA: f32 = 0.92;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// The design's tokens.
    pub c: Colors,
    /// The dark appearance, where controls have no shadow.
    pub dark: bool,
}

/// Every colour token of spec §3.4, named for its role.
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub window_bg: Hsla,
    /// The sidebar's fill over the main window's blurred desktop (spec §5 step 18).
    pub sidebar_translucent: Hsla,
    /// The sidebar's group labels: 4.5:1 on `sidebar_translucent` whatever the desktop behind it.
    pub sidebar_label: Hsla,
    pub content_bg: Hsla,
    pub surface: Hsla,
    pub surface_sunken: Hsla,
    pub control_bg: Hsla,
    pub control_border: Hsla,
    pub separator: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub accent: Hsla,
    pub accent_fill: Hsla,
    /// An accent pill's text (`accent` itself is too light on its wash).
    pub accent_text: Hsla,
    pub on_accent: Hsla,
    pub selection: Hsla,
    pub row_selected: Hsla,
    pub hover: Hsla,
    pub focus_ring: Hsla,
    pub success: Hsla,
    pub success_text: Hsla,
    pub warning: Hsla,
    pub warning_text: Hsla,
    pub danger: Hsla,
    pub danger_text: Hsla,
    pub danger_fill: Hsla,
    pub neutral: Hsla,
    pub neutral_text: Hsla,
    /// A switch's track while off.
    pub switch_off: Hsla,
    /// Behind a sheet.
    pub backdrop: Hsla,
    /// A control's 1 px bottom shadow (light only).
    pub control_shadow: Hsla,
}

/// What a state means, for its colour: a pill, a badge, an error bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Danger,
    Neutral,
    Accent,
}

/// `hex` at `alpha`.
fn a(hex: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = rgb(hex).into();
    c.a = alpha;
    c
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    pub fn of(window: &Window) -> Theme {
        match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::light(),
        }
    }

    pub fn light() -> Theme {
        let accent = c(0x1f7a4d);
        Theme {
            c: Colors {
                window_bg: c(0xf5f5f7),
                sidebar_translucent: a(0xededef, SIDEBAR_ALPHA),
                sidebar_label: c(0x58585d),
                content_bg: c(0xffffff),
                surface: c(0xffffff),
                surface_sunken: c(0xf5f5f7),
                control_bg: c(0xffffff),
                control_border: a(0x000000, 0.14),
                separator: a(0x000000, 0.09),
                text: c(0x1d1d1f),
                text_secondary: c(0x6a6a6f),
                text_tertiary: c(0xa1a1a6),
                accent,
                accent_fill: accent,
                accent_text: c(0x0e693c),
                on_accent: c(0xffffff),
                selection: a(0x000000, 0.08),
                row_selected: Theme::wash(accent, 0.14),
                hover: a(0x000000, 0.04),
                focus_ring: Theme::wash(accent, 0.40),
                success: c(0x34c759),
                success_text: c(0x137029),
                warning: c(0xff9500),
                warning_text: c(0x944f00),
                danger: c(0xff3b30),
                danger_text: c(0xb5190d),
                danger_fill: c(0xd93025),
                neutral: c(0x8e8e93),
                neutral_text: c(0x5f5f64),
                switch_off: c(0xd1d1d6),
                backdrop: a(0x000000, 0.25),
                control_shadow: a(0x000000, 0.05),
            },
            dark: false,
        }
    }

    pub fn dark() -> Theme {
        let accent = c(0x4cc38a);
        Theme {
            c: Colors {
                window_bg: c(0x1e1e1e),
                sidebar_translucent: a(0x262628, SIDEBAR_ALPHA),
                sidebar_label: c(0xb4b4b9),
                content_bg: c(0x1e1e1e),
                surface: c(0x2a2a2c),
                surface_sunken: c(0x161617),
                control_bg: c(0x3a3a3c),
                control_border: a(0xffffff, 0.10),
                separator: a(0xffffff, 0.09),
                text: c(0xf2f2f4),
                text_secondary: c(0x98989d),
                text_tertiary: c(0x6c6c70),
                accent,
                accent_fill: c(0x23794f),
                accent_text: c(0x5bd299),
                on_accent: c(0xffffff),
                selection: a(0xffffff, 0.10),
                row_selected: Theme::wash(accent, 0.20),
                hover: a(0xffffff, 0.05),
                focus_ring: Theme::wash(accent, 0.50),
                success: c(0x30d158),
                success_text: c(0x4cd964),
                warning: c(0xff9f0a),
                warning_text: c(0xffb340),
                danger: c(0xff453a),
                danger_text: c(0xff8e86),
                danger_fill: c(0xc9372c),
                neutral: c(0x98989d),
                neutral_text: c(0xb8b8bc),
                switch_off: c(0x48484a),
                backdrop: a(0x000000, 0.25),
                control_shadow: a(0x000000, 0.0),
            },
            dark: true,
        }
    }

    /// `color` at `alpha`: a banner's or a pill's background, a selected row.
    pub fn wash(color: Hsla, alpha: f32) -> Hsla {
        hsla(color.h, color.s, color.l, alpha)
    }

    /// `color` darkened by `by` (0.04 for a hover, 0.08 for a press).
    pub fn darker(color: Hsla, by: f32) -> Hsla {
        hsla(color.h, color.s, (color.l - by).max(0.), color.a)
    }

    /// A state's dot, text and wash (the wash at .15, spec §3.4).
    pub fn state(&self, tone: Tone) -> (Hsla, Hsla, Hsla) {
        let c = &self.c;
        let (dot, text) = match tone {
            Tone::Success => (c.success, c.success_text),
            Tone::Warning => (c.warning, c.warning_text),
            Tone::Danger => (c.danger, c.danger_text),
            Tone::Neutral => (c.neutral, c.neutral_text),
            Tone::Accent => (c.accent, c.accent_text),
        };
        (dot, text, Theme::wash(dot, 0.15))
    }
}

/// Spacing (spec §3.3).
pub mod space {
    use gpui::{Pixels, px};
    pub const XS: Pixels = px(4.);
    pub const SM: Pixels = px(8.);
    pub const MD: Pixels = px(12.);
    pub const LG: Pixels = px(16.);
    pub const XL: Pixels = px(20.);
    pub const XXL: Pixels = px(24.);
}

/// Corner radii (spec §3.3).
pub mod radius {
    use gpui::{Pixels, px};
    pub const SM: Pixels = px(4.);
    pub const MD: Pixels = px(6.);
    pub const LG: Pixels = px(8.);
    pub const XL: Pixels = px(10.);
}

/// Control heights and widths (spec §3.3).
pub mod size {
    use gpui::{Pixels, px};
    pub const CONTROL: Pixels = px(24.);
    pub const CONTROL_SMALL: Pixels = px(20.);
    pub const ROW: Pixels = px(28.);
    pub const ROW_ACTIONS: Pixels = px(32.);
    pub const SIDEBAR: Pixels = px(200.);
    pub const SIDEBAR_ITEM: Pixels = px(28.);
    pub const HEADER: Pixels = px(24.);
    pub const TOOLBAR: Pixels = px(52.);
    pub const FIELD: Pixels = px(180.);
    pub const PICKER_MIN: Pixels = px(120.);
    pub const SHEET: Pixels = px(460.);
}

/// The type scale (spec §3.2) as a `Styled` extension: a call site says `.type_body()`. Each sets
/// its own line height, so GPUI's φ never applies.
pub trait Typography: Styled + Sized {
    fn type_style(self, size: f32, line: f32, weight: f32) -> Self {
        self.text_size(px(size)).line_height(px(line)).font_weight(FontWeight(weight))
    }
    fn type_title(self) -> Self {
        self.type_style(20., 26., 600.)
    }
    fn type_section(self) -> Self {
        self.type_style(13., 18., 600.)
    }
    fn type_body(self) -> Self {
        self.type_style(13., 18., 400.)
    }
    fn type_body_strong(self) -> Self {
        self.type_style(13., 18., 500.)
    }
    fn type_secondary(self) -> Self {
        self.type_style(12., 16., 400.)
    }
    fn type_caption(self) -> Self {
        self.type_style(11., 14., 500.)
    }
    /// Menlo: GPUI takes one family, and Menlo is on every Mac (the spec's SF Mono is not a
    /// family name an app can ask for).
    fn type_mono(self) -> Self {
        self.type_style(12., 17., 400.).font_family("Menlo")
    }
}

impl<T: Styled + Sized> Typography for T {}

/// WCAG relative luminance of an opaque colour.
#[cfg(test)]
fn luminance(c: Hsla) -> f32 {
    let Rgba { r, g, b, .. } = c.into();
    let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// `top` composited over the opaque `under`.
#[cfg(test)]
fn over(top: Hsla, under: Hsla) -> Hsla {
    let (t, u): (Rgba, Rgba) = (top.into(), under.into());
    let mix = |x: f32, y: f32| x * t.a + y * (1. - t.a);
    Rgba { r: mix(t.r, u.r), g: mix(t.g, u.g), b: mix(t.b, u.b), a: 1. }.into()
}

#[cfg(test)]
pub(crate) fn contrast(fg: Hsla, bg: Hsla) -> f32 {
    let (a, b) = (luminance(fg), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text colour meets 4.5:1 on the content background, and each state's text on its own
    /// pill wash (spec §3.4).
    #[test]
    fn text_tokens_meet_contrast() {
        for (name, t) in [("light", Theme::light()), ("dark", Theme::dark())] {
            let c = t.c;
            let bg = c.content_bg;
            for (token, fg) in [
                ("text", c.text),
                ("text_secondary", c.text_secondary),
                ("success_text", c.success_text),
                ("warning_text", c.warning_text),
                ("danger_text", c.danger_text),
                ("neutral_text", c.neutral_text),
            ] {
                let r = contrast(fg, bg);
                assert!(r >= 4.5, "{name} {token} on content_bg is {r:.2}");
            }
            // A pill sits on the content, or on a selected row's wash (the PHP table always has one).
            let selected = over(c.row_selected, bg);
            for tone in [Tone::Success, Tone::Warning, Tone::Danger, Tone::Neutral, Tone::Accent] {
                let (_, text, wash) = t.state(tone);
                for (under, on) in [("content", bg), ("a selected row", selected)] {
                    let r = contrast(text, over(wash, on));
                    assert!(r >= 4.5, "{name} {tone:?} text on its wash over {under} is {r:.2}");
                }
            }
            // The sidebar lets the blurred desktop through: its labels and items stay readable over
            // a black and a white desktop alike (a dark Mac's labels read 3.9:1 in text_secondary).
            for (desk, under) in [("black", super::c(0x000000)), ("white", super::c(0xffffff))] {
                let bg = over(c.sidebar_translucent, under);
                for (token, fg) in [("sidebar_label", c.sidebar_label), ("text", c.text)] {
                    let r = contrast(fg, bg);
                    assert!(r >= 4.5, "{name} {token} on the sidebar over a {desk} desktop is {r:.2}");
                }
            }
            let r = contrast(c.on_accent, c.accent_fill);
            assert!(r >= 4.5, "{name} on_accent on accent_fill is {r:.2}");
            let r = contrast(c.on_accent, c.danger_fill);
            assert!(r >= 4.5, "{name} on_accent on danger_fill is {r:.2}");
        }
    }

    #[test]
    fn light_and_dark_tokens_match_the_spec() {
        let hex = |c: Hsla| {
            let Rgba { r, g, b, .. } = c.into();
            ((r * 255.).round() as u32) << 16 | ((g * 255.).round() as u32) << 8 | (b * 255.).round() as u32
        };
        let (l, d) = (Theme::light().c, Theme::dark().c);
        assert_eq!((hex(l.accent), hex(d.accent)), (0x1f7a4d, 0x4cc38a));
        assert_eq!((hex(l.danger_fill), hex(d.danger_fill)), (0xd93025, 0xc9372c));
        assert_eq!((hex(l.window_bg), hex(d.window_bg)), (0xf5f5f7, 0x1e1e1e));
        assert_eq!(hex(d.accent_fill), 0x23794f);
    }
}
