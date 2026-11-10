//! Encode GPUI keystrokes that are handled by the terminal rather than text input.

use gpui::Keystroke;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TerminalKey {
    Input(String),
    ScrollPage(i32),
}

pub(crate) fn terminal_key(
    key: &Keystroke,
    application_cursor: bool,
    is_mac: bool,
) -> Option<TerminalKey> {
    let modifiers = key.modifiers;
    if modifiers.platform {
        return None;
    }
    let key = &key.key;
    if modifiers.shift && matches!(key.as_str(), "pageup" | "pagedown") {
        return Some(TerminalKey::ScrollPage(if key == "pageup" {
            -1
        } else {
            1
        }));
    }
    let modifier_code = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    let data = match key.as_str() {
        "enter" if modifiers.alt => "\x1b\r".into(),
        "enter" => "\r".into(),
        "backspace" => {
            let backspace = if modifiers.control { '\u{8}' } else { '\u{7f}' };
            if modifiers.alt {
                format!("\x1b{backspace}")
            } else {
                backspace.to_string()
            }
        }
        "escape" if modifiers.alt => "\x1b\x1b".into(),
        "escape" => "\x1b".into(),
        "tab" if modifiers.shift => "\x1b[Z".into(),
        "tab" => "\t".into(),
        "up" | "down" | "right" | "left" | "home" | "end" => {
            let final_byte = match key.as_str() {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                _ => 'F',
            };
            if modifier_code != 1 {
                format!("\x1b[1;{modifier_code}{final_byte}")
            } else if application_cursor {
                format!("\x1bO{final_byte}")
            } else {
                format!("\x1b[{final_byte}")
            }
        }
        "insert" if modifiers.shift || modifiers.control => return None,
        "insert" => "\x1b[2~".into(),
        "delete" | "pageup" | "pagedown" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11"
        | "f12" => {
            let code = match key.as_str() {
                "delete" => 3,
                "pageup" => 5,
                "pagedown" => 6,
                "f5" => 15,
                "f6" => 17,
                "f7" => 18,
                "f8" => 19,
                "f9" => 20,
                "f10" => 21,
                "f11" => 23,
                _ => 24,
            };
            if modifier_code == 1
                || (matches!(key.as_str(), "pageup" | "pagedown") && !modifiers.control)
            {
                format!("\x1b[{code}~")
            } else {
                format!("\x1b[{code};{modifier_code}~")
            }
        }
        "f1" | "f2" | "f3" | "f4" => {
            let final_byte = char::from(b'P' + key.as_bytes()[1] - b'1');
            if modifier_code == 1 {
                format!("\x1bO{final_byte}")
            } else {
                format!("\x1b[1;{modifier_code}{final_byte}")
            }
        }
        "space" if modifiers.control && !modifiers.alt && !modifiers.shift => "\0".into(),
        "space" if modifiers.alt && !modifiers.control && !is_mac => "\x1b ".into(),
        _ if modifiers.control && modifiers.shift && !modifiers.alt => match key.as_str() {
            "-" | "_" => "\x1f".into(),
            "2" | "@" => "\0".into(),
            "6" | "^" => "\x1e".into(),
            _ => return None,
        },
        _ if modifiers.control && key.len() == 1 && !modifiers.alt => {
            let byte = key.as_bytes()[0].to_ascii_uppercase();
            match byte {
                b'@'..=b'_' => char::from(byte & 0x1f).to_string(),
                b'3'..=b'7' => char::from(byte - b'3' + 0x1b).to_string(),
                b'/' => "\x1f".into(),
                b'8' | b'?' => "\u{7f}".into(),
                _ => return None,
            }
        }
        _ if modifiers.alt && !modifiers.control && !is_mac && key.len() == 1 => {
            let byte = key.as_bytes()[0];
            let ch = if modifiers.shift {
                byte.to_ascii_uppercase()
            } else {
                byte
            };
            format!("\x1b{}", char::from(ch))
        }
        _ => return None,
    };
    Some(TerminalKey::Input(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_control_keys_preserve_text_input_and_app_shortcuts() {
        let check = |key| terminal_key(&Keystroke::parse(key).unwrap(), false, true);
        for (key, data) in [
            ("ctrl-c", "\x03"),
            ("ctrl-d", "\x04"),
            ("ctrl-3", "\x1b"),
            ("tab", "\t"),
            ("alt-enter", "\x1b\r"),
            ("backspace", "\x7f"),
        ] {
            assert_eq!(check(key), Some(TerminalKey::Input(data.into())));
        }
        for key in ["cmd-w", "ctrl-shift-c", "a", "alt-a"] {
            assert_eq!(check(key), None);
        }
    }

    #[test]
    fn terminal_arrows_and_paging_use_the_active_mode() {
        let check = |key, mode| terminal_key(&Keystroke::parse(key).unwrap(), mode, true);
        assert_eq!(
            check("up", false),
            Some(TerminalKey::Input("\x1b[A".into()))
        );
        assert_eq!(check("up", true), Some(TerminalKey::Input("\x1bOA".into())));
        assert_eq!(
            check("ctrl-left", true),
            Some(TerminalKey::Input("\x1b[1;5D".into()))
        );
        assert_eq!(
            check("shift-pageup", true),
            Some(TerminalKey::ScrollPage(-1))
        );
    }
}
