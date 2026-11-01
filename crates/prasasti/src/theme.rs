use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont};

/// The editor colours for one appearance.
pub struct Palette {
    pub bg: Retained<NSColor>,
    pub text: Retained<NSColor>,
    pub accent: Retained<NSColor>,
    pub marker: Retained<NSColor>,
    pub code_bg: Retained<NSColor>,
    pub selection: Retained<NSColor>,
    pub search: Retained<NSColor>,
    pub current: Retained<NSColor>,
}

fn rgb(hex: u32) -> Retained<NSColor> {
    let c = |shift: u32| f64::from((hex >> shift) & 0xff) / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(c(16), c(8), c(0), 1.0)
}

/// The palette as sRGB hex, in `Palette` field order (Preview's CSS uses it too).
pub fn hex(dark: bool) -> [u32; 8] {
    if dark {
        [0x101010, 0xeeeeee, 0x5584aa, 0x4f525a, 0x1d1d1d, 0x186a9a, 0x725b18, 0xb36b20]
    } else {
        [0xffffff, 0x222324, 0x2077b2, 0xaeb1b5, 0xf2f2f2, 0x2077b2, 0xffe58a, 0xffad42]
    }
}

pub fn palette(dark: bool) -> Palette {
    let [bg, text, accent, marker, code_bg, selection, search, current] = hex(dark).map(rgb);
    Palette { bg, text, accent, marker, code_bg, selection, search, current }
}

/// The editor font, from the font and zoom settings.
pub fn base_font() -> Retained<NSFont> {
    crate::settings::current_font()
}
