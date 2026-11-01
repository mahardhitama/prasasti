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
        // Catppuccin Mocha: base, text, blue, overlay0, mantle, surface1,
        // yellow 30% and peach 45% over base (highlights must be opaque).
        [0x1e1e2e, 0xcdd6f4, 0x89b4fa, 0x6c7086, 0x181825, 0x45475a, 0x605955, 0x816156]
    } else {
        // Catppuccin Latte, same roles; yellow 30% and peach 40% over base.
        [0xeff1f5, 0x4c4f69, 0x1e66f5, 0x9ca0b0, 0xe6e9ef, 0xbcc0cc, 0xead3b4, 0xf5b897]
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
