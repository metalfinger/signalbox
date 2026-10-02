//! The terminal palette: Ghostty's "Terminal Basic Dark", on black.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

pub const BACKGROUND: u32 = 0x000000;
pub const FOREGROUND: u32 = 0xffffff;
pub const CURSOR: u32 = 0x9d9d9d;
/// Behind selected text.
pub const SELECTION: u32 = 0x24414d;

const ANSI: [u32; 16] = [
    0x000000, 0xc65339, 0x6ac44b, 0xb8b74a, 0x6444ed, 0xd357db, 0x69c1cf, 0xd1d1d1, 0x909090, 0xeb5a3a, 0x77ea51,
    0xefef53, 0xd09af9, 0xeb5af7, 0x78f1f2, 0xededed,
];

/// The 16 named colours, the 6×6×6 cube and the grey ramp.
pub fn indexed(index: u8) -> u32 {
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            const STEPS: [u32; 6] = [0, 95, 135, 175, 215, 255];
            let i = (index - 16) as usize;
            (STEPS[i / 36] << 16) | (STEPS[(i / 6) % 6] << 8) | STEPS[i % 6]
        }
        232..=255 => {
            let v = 8 + 10 * (index as u32 - 232);
            (v << 16) | (v << 8) | v
        }
    }
}

/// Faint text (SGR 2), as Ghostty draws it: halfway to the background behind it.
pub fn faint(color: u32, background: u32) -> u32 {
    let mix = |shift: u32| ((((color >> shift) & 0xff) + ((background >> shift) & 0xff)) / 2) << shift;
    mix(16) | mix(8) | mix(0)
}

fn dim(color: u32) -> u32 {
    let scale = |shift: u32| ((((color >> shift) & 0xff) * 2 / 3) & 0xff) << shift;
    scale(16) | scale(8) | scale(0)
}

fn named(name: NamedColor) -> u32 {
    let index = name as usize;
    match name {
        NamedColor::Foreground | NamedColor::BrightForeground => FOREGROUND,
        NamedColor::DimForeground => dim(FOREGROUND),
        NamedColor::Background => BACKGROUND,
        NamedColor::Cursor => CURSOR,
        _ if index < 16 => ANSI[index],
        // DimBlack..DimWhite follow Cursor.
        _ if (259..267).contains(&index) => dim(ANSI[index - 259]),
        _ => FOREGROUND,
    }
}

fn from_rgb(rgb: Rgb) -> u32 {
    ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
}

pub fn to_rgb(color: u32) -> Rgb {
    Rgb {
        r: (color >> 16) as u8,
        g: (color >> 8) as u8,
        b: color as u8,
    }
}

/// A cell colour as 0xRRGGBB. Colours a program set with OSC 4/10/11 win over the palette.
pub fn resolve(color: Color, overrides: &Colors) -> u32 {
    match color {
        Color::Spec(rgb) => from_rgb(rgb),
        Color::Indexed(index) => overrides[index as usize]
            .map(from_rgb)
            .unwrap_or_else(|| indexed(index)),
        Color::Named(name) => overrides[name].map(from_rgb).unwrap_or_else(|| named(name)),
    }
}

/// The colour a program asked about with OSC 4/10/11/12 (index as alacritty numbers them).
pub fn for_request(index: usize, overrides: &Colors) -> Rgb {
    if let Some(rgb) = overrides[index] {
        return rgb;
    }
    to_rgb(match index {
        0..=255 => indexed(index as u8),
        256 => FOREGROUND,
        257 => BACKGROUND,
        258 => CURSOR,
        _ => FOREGROUND,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faint_text_is_halfway_to_its_background() {
        assert_eq!(faint(0xffffff, 0x000000), 0x7f7f7f);
        assert_eq!(faint(0xd1d1d1, 0x101010), 0x707070);
    }

    #[test]
    fn cube_and_greys() {
        assert_eq!(indexed(1), 0xc65339);
        assert_eq!(indexed(16), 0x000000);
        assert_eq!(indexed(196), 0xff0000);
        assert_eq!(indexed(231), 0xffffff);
        assert_eq!(indexed(232), 0x080808);
        assert_eq!(indexed(255), 0xeeeeee);
    }

    #[test]
    fn named_colors() {
        let none = Colors::default();
        assert_eq!(resolve(Color::Named(NamedColor::Foreground), &none), FOREGROUND);
        assert_eq!(resolve(Color::Named(NamedColor::Background), &none), BACKGROUND);
        assert_eq!(resolve(Color::Named(NamedColor::BrightRed), &none), 0xeb5a3a);
        assert_eq!(resolve(Color::Named(NamedColor::DimWhite), &none), dim(0xd1d1d1));
        assert_eq!(resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &none), 0x010203);
    }
}
