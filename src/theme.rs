//! Champagne: warm blacks, warm-white text, a pale steel for work in progress, and champagne
//! for whatever needs you (and for the app's own accent). The sidebar, title bar and status
//! bar are glass: the desktop shows through them, blurred. The terminal stays opaque.
//!
//! These constants are the one palette: the kit's components get the same values (`apply`).

use std::borrow::Cow;
use std::rc::Rc;

use gpui::App;
use gpui_kit::component::{Theme, ThemeSet};

/// Opaque ground: the terminal's surround and the start page.
pub const BG: u32 = 0x0d0d0e;
pub const PANEL: u32 = 0x141415;
pub const SURFACE: u32 = 0x1c1c1e;
pub const SURFACE_HOVER: u32 = 0x242427;
pub const BORDER: u32 = 0x2a2a2d;
/// An unlit state light, and the empty part of a meter.
pub const LED_OFF: u32 = 0x3a3a3e;
pub const TEXT: u32 = 0xeeeae3;
pub const TEXT_MUTED: u32 = 0xaba59c;
/// The quietest text that still reads (4.5:1 on the panels).
pub const TEXT_FAINT: u32 = 0x8c867e;
/// Champagne: the accent, and whatever needs you.
pub const ACCENT: u32 = 0xd9b77e;
pub const ACCENT_HOVER: u32 = 0xe6c995;
/// Text on a champagne fill.
pub const ON_ACCENT: u32 = 0x17130c;
/// Pale steel: work in progress.
pub const WORKING: u32 = 0x9db3c9;
pub const OK: u32 = 0x9bc4a5;
pub const ERR: u32 = 0xe38a7a;

/// Glass, as RRGGBBAA: a warm black with a hint of the blurred desktop through it. GPUI's blur is
/// colourless, so this sets the tone; much less opaque and a bright wallpaper turns it grey.
pub const GLASS: u32 = 0x121213e0;
/// On glass: a hovered row, and the selected one (white over whatever is behind).
pub const GLASS_HOVER: u32 = 0xffffff0f;
pub const GLASS_SELECTED: u32 = 0xffffff1c;

/// The brand's wordmark.
pub const WORDMARK: &str = "Martian Mono SemiCondensed";
/// Code and commands, in the terminal's font.
pub const CODE: &str = "Hack Nerd Font Mono";

/// Martian Mono ships with the app for the wordmark (SIL Open Font License, assets/fonts/OFL.txt).
pub fn load_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/MartianMono-SemiCondensed-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/MartianMono-SemiCondensed-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/MartianMono-SemiCondensed-SemiBold.ttf")),
    ];
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        eprintln!("[theme] couldn't load Martian Mono: {error}");
    }
}

fn hex(color: u32) -> String {
    format!("#{color:06x}")
}

fn hex_alpha(color: u32) -> String {
    format!("#{color:08x}")
}

/// The kit's theme in the same palette, as its own theme file would say it.
fn kit_theme() -> String {
    let colors = [
        ("background", hex_alpha(GLASS)),
        ("foreground", hex(TEXT)),
        ("border", hex(BORDER)),
        ("window.border", hex(BORDER)),
        ("overlay", "#00000099".into()),
        ("muted.background", hex(SURFACE)),
        ("muted.foreground", hex(TEXT_MUTED)),
        ("accent.background", hex(SURFACE_HOVER)),
        ("accent.foreground", hex(TEXT)),
        ("primary.background", hex(ACCENT)),
        ("primary.foreground", hex(ON_ACCENT)),
        ("primary.hover.background", hex(ACCENT_HOVER)),
        ("primary.active.background", "#c9a66c".into()),
        ("secondary.background", hex(SURFACE)),
        ("secondary.foreground", hex(TEXT)),
        ("secondary.hover.background", hex(SURFACE_HOVER)),
        ("secondary.active.background", "#2c2c30".into()),
        ("button.background", hex(SURFACE)),
        ("button.foreground", hex(TEXT)),
        ("button.hover.background", hex(SURFACE_HOVER)),
        ("button.active.background", "#2c2c30".into()),
        ("button.primary.background", hex(ACCENT)),
        ("button.primary.foreground", hex(ON_ACCENT)),
        ("button.primary.hover.background", hex(ACCENT_HOVER)),
        ("button.primary.active.background", "#c9a66c".into()),
        ("ring", hex(ACCENT)),
        ("caret", hex(ACCENT)),
        ("selection.background", "#3a342a".into()),
        ("input.border", hex(BORDER)),
        ("link", hex(ACCENT)),
        ("link.hover", hex(ACCENT_HOVER)),
        ("link.active", "#c9a66c".into()),
        ("popover.background", "#1a1a1c".into()),
        ("popover.foreground", hex(TEXT)),
        ("list.background", "#00000000".into()),
        ("list.hover.background", hex_alpha(GLASS_HOVER)),
        ("list.active.background", "#2a2620".into()),
        ("list.active.border", hex(ACCENT)),
        ("scrollbar.background", "#00000000".into()),
        ("scrollbar.thumb.background", "#ffffff26".into()),
        ("scrollbar.thumb.hover.background", "#ffffff40".into()),
        ("title_bar.background", "#00000000".into()),
        ("title_bar.border", hex(BORDER)),
        ("status_bar.background", "#00000000".into()),
        ("status_bar.border", hex(BORDER)),
        ("sidebar.background", "#00000000".into()),
        ("sidebar.foreground", hex(TEXT)),
        ("sidebar.border", hex(BORDER)),
        ("sidebar.accent.background", hex_alpha(GLASS_SELECTED)),
        ("sidebar.accent.foreground", hex(TEXT)),
        ("tab.background", "#00000000".into()),
        ("tab.foreground", hex(TEXT_MUTED)),
        ("tab.active.background", hex(SURFACE)),
        ("tab.active.foreground", hex(TEXT)),
        ("tab_bar.background", "#00000000".into()),
        ("drag.border", hex(ACCENT)),
        ("drop_target.background", "#d9b77e1a".into()),
        ("warning.background", hex(ACCENT)),
        ("warning.foreground", hex(ON_ACCENT)),
        ("danger.background", hex(ERR)),
        ("danger.foreground", "#1b0f0c".into()),
        ("success.background", hex(OK)),
        ("success.foreground", "#0f1a12".into()),
        ("info.background", hex(WORKING)),
        ("info.foreground", "#0e141a".into()),
        ("base.red", hex(ERR)),
        ("base.green", hex(OK)),
        ("base.blue", hex(WORKING)),
        ("base.yellow", hex(ACCENT)),
    ];
    let colors = colors
        .iter()
        .map(|(key, value)| format!("\"{key}\": \"{value}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"{{"name": "Signalbox", "themes": [{{"name": "Champagne", "mode": "dark",
            "font.family": ".SystemUIFont", "font.size": 16, "mono_font.family": "{CODE}", "mono_font.size": 13,
            "radius": 8, "radius.lg": 10, "shadow": true, "colors": {{ {colors} }} }}]}}"#
    )
}

/// The whole app is dark whatever the system's setting, so the glass blurs with the dark
/// material (like Finder's sidebar in dark mode) and the kit never switches to its light theme.
pub fn use_dark_appearance() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // Safety: plain AppKit calls on the main thread, with a valid NSString.
    unsafe {
        let name: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c"NSAppearanceNameDarkAqua".as_ptr()];
        let appearance: *mut Object = msg_send![class!(NSAppearance), appearanceNamed: name];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if !appearance.is_null() {
            let _: () = msg_send![app, setAppearance: appearance];
        }
    }
}

/// Dresses the kit's components in this palette. The kit starts out light.
pub fn apply(cx: &mut App) {
    let theme = match serde_json::from_str::<ThemeSet>(&kit_theme()) {
        Ok(set) => set.themes.into_iter().next(),
        Err(error) => {
            eprintln!("[theme] the kit theme didn't parse: {error}");
            None
        }
    };
    if let Some(theme) = theme {
        let theme = Rc::new(theme);
        Theme::update(cx, |kit| kit.apply_config(&theme));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_kit_theme_parses() {
        let set: gpui_kit::component::ThemeSet = serde_json::from_str(&super::kit_theme()).unwrap();
        assert_eq!(set.themes.len(), 1);
    }
}
