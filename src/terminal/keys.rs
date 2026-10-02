//! What a key press or mouse event sends to the program in the terminal, following xterm.

use alacritty_terminal::term::TermMode;
use gpui::{Keystroke, Modifiers};

/// xterm's modifier parameter: 1 + Shift + 2·Alt + 4·Ctrl.
fn modifier_code(m: &Modifiers) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8
}

/// Plain typing: printable text with no Control, Option, Cmd or Fn. macOS delivers it through
/// its text input, which is what makes accents, emoji, dictation and input methods work; the
/// view's input handler writes it to the program.
pub fn is_text_input(keystroke: &Keystroke) -> bool {
    let m = &keystroke.modifiers;
    !(m.control || m.alt || m.platform || m.function)
        && keystroke
            .key_char
            .as_ref()
            .is_some_and(|text| !text.is_empty() && !text.chars().any(char::is_control))
}

/// Bytes for a key press, or `None` when the key belongs to the app (Cmd shortcuts) or sends
/// nothing. Option acts as Alt (an Esc prefix).
pub fn key_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    if let Some(bytes) = text_editing(keystroke) {
        return Some(bytes.to_vec());
    }
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    let mods = modifier_code(m);
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let cursor = |c: char| {
        if mods > 1 {
            format!("\x1b[1;{mods}{c}")
        } else if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
        .into_bytes()
    };
    let function = |c: char| {
        if mods > 1 {
            format!("\x1b[1;{mods}{c}")
        } else {
            format!("\x1bO{c}")
        }
        .into_bytes()
    };
    let tilde = |n: u8| {
        if mods > 1 {
            format!("\x1b[{n};{mods}~")
        } else {
            format!("\x1b[{n}~")
        }
        .into_bytes()
    };
    let alt = |bytes: &[u8]| {
        let mut out = if m.alt { vec![0x1b] } else { Vec::new() };
        out.extend_from_slice(bytes);
        out
    };

    let bytes = match keystroke.key.as_str() {
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "delete" => tilde(3),
        "insert" => tilde(2),
        "f1" => function('P'),
        "f2" => function('Q'),
        "f3" => function('R'),
        "f4" => function('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "enter" => alt(b"\r"),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => alt(b"\t"),
        "backspace" if m.control => vec![0x08],
        "backspace" => alt(&[0x7f]),
        "escape" => alt(&[0x1b]),
        "space" if m.control => alt(&[0]),
        "space" => alt(b" "),
        key if m.control => {
            let mut out = if m.alt { vec![0x1b] } else { Vec::new() };
            out.push(control_byte(key)?);
            out
        }
        key if m.alt => {
            // Esc + the key itself, not the symbol macOS would type for Option+key.
            if m.shift && key.chars().count() == 1 {
                alt(key.to_uppercase().as_bytes())
            } else {
                alt(key.as_bytes())
            }
        }
        key => match &keystroke.key_char {
            Some(text) => text.as_bytes().to_vec(),
            None if key.chars().count() == 1 => key.as_bytes().to_vec(),
            None => return None,
        },
    };
    Some(bytes)
}

/// Editing the line being typed the Mac way, as Ghostty sets it up: Cmd goes to or deletes to
/// the line's ends, Option moves or deletes by words. Sent as the keys shells and Claude Code
/// read that way (Ctrl-A, Ctrl-E, Ctrl-U, Ctrl-K, Esc-b, Esc-f, Esc-d); Option-Backspace is
/// already Esc-Delete.
fn text_editing(keystroke: &Keystroke) -> Option<&'static [u8]> {
    let m = &keystroke.modifiers;
    if m.control || m.shift {
        return None;
    }
    match (m.platform, m.alt, keystroke.key.as_str()) {
        (true, false, "left") => Some(b"\x01"),
        (true, false, "right") => Some(b"\x05"),
        (true, false, "backspace") => Some(b"\x15"),
        (true, false, "delete") => Some(b"\x0b"),
        (false, true, "left") => Some(b"\x1bb"),
        (false, true, "right") => Some(b"\x1bf"),
        (false, true, "delete") => Some(b"\x1bd"),
        _ => None,
    }
}

fn control_byte(key: &str) -> Option<u8> {
    let byte = match key {
        k if k.len() == 1 && k.as_bytes()[0].is_ascii_alphabetic() => k.as_bytes()[0].to_ascii_lowercase() - b'a' + 1,
        "@" | "2" => 0,
        "[" | "3" => 0x1b,
        "\\" | "4" => 0x1c,
        "]" | "5" => 0x1d,
        "^" | "6" => 0x1e,
        "_" | "-" | "7" | "/" => 0x1f,
        "8" | "?" => 0x7f,
        _ => return None,
    };
    Some(byte)
}

/// Text pasted into the terminal: line breaks become Enter, bracketed when the program asked for it.
pub fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // A pasted end marker must not end the paste early.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

/// A matched link without what follows a URL in prose: trailing punctuation, and closing
/// brackets it didn't open (`(see https://x.com/a)`).
pub fn trim_link(text: &str) -> &str {
    let mut link = text;
    while let Some(last) = link.chars().last() {
        let unbalanced = |open: char, close: char| last == close && link.matches(open).count() < link.matches(close).count();
        if ".,:;!?'\"".contains(last) || unbalanced('(', ')') || unbalanced('[', ']') || unbalanced('{', '}') {
            link = &link[..link.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    link
}

/// A file's path as typed into a shell, the way Terminal and Ghostty drop files: every
/// character a shell treats specially gets a backslash.
pub fn escape_path(path: &std::path::Path) -> String {
    let mut escaped = String::new();
    for c in path.to_string_lossy().chars() {
        if !(c.is_alphanumeric() || "/._-+,@%=:".contains(c)) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press(u8),
    Release(u8),
    Drag(u8),
    Move,
    WheelUp,
    WheelDown,
}

/// Mouse report for a cell (0-based), when the program turned mouse reporting on (tmux does with `mouse on`).
pub fn mouse_bytes(action: MouseAction, col: usize, row: usize, m: &Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut code = match action {
        MouseAction::Press(button) => button,
        MouseAction::Release(button) => {
            if sgr {
                button
            } else {
                3
            }
        }
        MouseAction::Drag(button) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => button + 32,
        MouseAction::Move if mode.contains(TermMode::MOUSE_MOTION) => 35,
        MouseAction::Drag(_) | MouseAction::Move => return None,
        MouseAction::WheelUp => 64,
        MouseAction::WheelDown => 65,
    };
    if m.shift {
        code += 4;
    }
    if m.alt {
        code += 8;
    }
    if m.control {
        code += 16;
    }
    if sgr {
        let end = if matches!(action, MouseAction::Release(_)) {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    let (x, y) = (col + 33, row + 33);
    if x > 255 || y > 255 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', 32 + code, x as u8, y as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(spec: &str) -> Option<Vec<u8>> {
        key_bytes(&Keystroke::parse(spec).unwrap(), TermMode::empty())
    }

    fn typed(key: &str, ch: &str) -> Option<Vec<u8>> {
        let mut keystroke = Keystroke::parse(key).unwrap();
        keystroke.key_char = Some(ch.to_string());
        key_bytes(&keystroke, TermMode::empty())
    }

    #[test]
    fn plain_and_shifted_text() {
        assert_eq!(typed("a", "a"), Some(b"a".to_vec()));
        assert_eq!(typed("shift-a", "A"), Some(b"A".to_vec()));
        assert_eq!(typed("!", "!"), Some(b"!".to_vec()));
    }

    #[test]
    fn control_and_alt() {
        assert_eq!(press("ctrl-c"), Some(vec![3]));
        assert_eq!(press("ctrl-b"), Some(vec![2]));
        assert_eq!(press("ctrl-["), Some(vec![0x1b]));
        assert_eq!(press("ctrl-alt-x"), Some(vec![0x1b, 0x18]));
        assert_eq!(press("alt-b"), Some(b"\x1bb".to_vec()));
        assert_eq!(press("alt-shift-b"), Some(b"\x1bB".to_vec()));
        assert_eq!(press("alt-backspace"), Some(vec![0x1b, 0x7f]));
    }

    #[test]
    fn named_keys() {
        assert_eq!(press("enter"), Some(b"\r".to_vec()));
        assert_eq!(press("escape"), Some(vec![0x1b]));
        assert_eq!(press("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(press("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(press("shift-up"), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(press("ctrl-left"), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(press("pagedown"), Some(b"\x1b[6~".to_vec()));
        assert_eq!(press("f5"), Some(b"\x1b[15~".to_vec()));
        assert_eq!(
            key_bytes(&Keystroke::parse("up").unwrap(), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn cmd_keys_stay_with_the_app() {
        assert_eq!(press("cmd-t"), None);
        assert_eq!(press("cmd-v"), None);
        assert_eq!(press("cmd-shift-left"), None);
    }

    #[test]
    fn line_editing_works_the_mac_way() {
        assert_eq!(press("cmd-left"), Some(vec![0x01]));
        assert_eq!(press("cmd-right"), Some(vec![0x05]));
        assert_eq!(press("cmd-backspace"), Some(vec![0x15]));
        assert_eq!(press("cmd-delete"), Some(vec![0x0b]));
        assert_eq!(press("alt-left"), Some(b"\x1bb".to_vec()));
        assert_eq!(press("alt-right"), Some(b"\x1bf".to_vec()));
        assert_eq!(press("alt-delete"), Some(b"\x1bd".to_vec()));
        // Arrows on their own and with Shift keep xterm's sequences.
        assert_eq!(press("left"), Some(b"\x1b[D".to_vec()));
        assert_eq!(press("alt-shift-left"), Some(b"\x1b[1;4D".to_vec()));
    }

    #[test]
    fn paste_is_bracketed_when_asked() {
        assert_eq!(paste_bytes("a\nb", TermMode::empty()), b"a\rb".to_vec());
        assert_eq!(
            paste_bytes("x", TermMode::BRACKETED_PASTE),
            b"\x1b[200~x\x1b[201~".to_vec()
        );
    }

    #[test]
    fn mouse_reports() {
        let none = Modifiers::default();
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert_eq!(mouse_bytes(MouseAction::Press(0), 0, 0, &none, TermMode::empty()), None);
        assert_eq!(
            mouse_bytes(MouseAction::Press(0), 4, 9, &none, sgr),
            Some(b"\x1b[<0;5;10M".to_vec())
        );
        assert_eq!(
            mouse_bytes(MouseAction::Release(0), 4, 9, &none, sgr),
            Some(b"\x1b[<0;5;10m".to_vec())
        );
        assert_eq!(
            mouse_bytes(MouseAction::Drag(0), 1, 1, &none, sgr),
            Some(b"\x1b[<32;2;2M".to_vec())
        );
        assert_eq!(
            mouse_bytes(MouseAction::WheelDown, 0, 0, &none, sgr),
            Some(b"\x1b[<65;1;1M".to_vec())
        );
        assert_eq!(mouse_bytes(MouseAction::Move, 0, 0, &none, sgr), None);
        let x10 = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            mouse_bytes(MouseAction::Press(0), 0, 0, &none, x10),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
    }

    #[test]
    fn dropped_paths_are_escaped_like_terminal_does() {
        use std::path::Path;
        assert_eq!(escape_path(Path::new("/Users/me/shot.png")), "/Users/me/shot.png");
        assert_eq!(
            escape_path(Path::new("/Users/me/Screen Shot (1)'s.png")),
            "/Users/me/Screen\\ Shot\\ \\(1\\)\\'s.png"
        );
        assert_eq!(escape_path(Path::new("/tmp/café-ü.jpg")), "/tmp/café-ü.jpg");
    }

    #[test]
    fn links_lose_what_prose_adds() {
        assert_eq!(trim_link("https://x.com/a."), "https://x.com/a");
        assert_eq!(trim_link("https://x.com/a),"), "https://x.com/a");
        assert_eq!(trim_link("https://en.wikipedia.org/wiki/Rust_(language)"), "https://en.wikipedia.org/wiki/Rust_(language)");
        assert_eq!(trim_link("https://x.com/?q=1"), "https://x.com/?q=1");
    }

    #[test]
    fn typing_goes_through_text_input_and_keys_dont() {
        let key = |spec: &str, key_char: Option<&str>| {
            let mut keystroke = Keystroke::parse(spec).unwrap();
            keystroke.key_char = key_char.map(str::to_string);
            is_text_input(&keystroke)
        };
        assert!(key("a", Some("a")));
        assert!(key("shift-a", Some("A")));
        assert!(key("space", Some(" ")));
        assert!(!key("enter", Some("\r")));
        assert!(!key("ctrl-c", Some("c")));
        assert!(!key("alt-b", Some("∫")));
        assert!(!key("up", None));
    }
}
