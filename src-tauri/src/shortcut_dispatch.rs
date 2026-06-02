use std::sync::{Mutex, OnceLock};

use crate::types::ShortcutBinding;

static SHORTCUT_DISPATCH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn shortcut_dispatch_lock() -> &'static Mutex<()> {
    SHORTCUT_DISPATCH_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn send_shortcut(binding: &ShortcutBinding) -> Result<(), String> {
    let _dispatch_guard = shortcut_dispatch_lock()
        .lock()
        .map_err(|_| "shortcut dispatch mutex poisoned".to_string())?;

    #[cfg(target_os = "windows")]
    {
        return windows_shortcut::send(binding);
    }

    #[cfg(not(target_os = "windows"))]
    {
        return send_shortcut_enigo(binding);
    }
}

#[cfg(not(target_os = "windows"))]
fn send_shortcut_enigo(binding: &ShortcutBinding) -> Result<(), String> {
    use enigo::{Direction, Enigo, Keyboard, Settings};

    let modifiers = shortcut_modifiers(binding)?;
    let primary = shortcut_primary(&binding.primary)?;
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    let mut pressed = 0usize;
    let mut first_err: Option<String> = None;

    for modifier in &modifiers {
        if let Err(e) = enigo.key(*modifier, Direction::Press) {
            first_err = Some(e.to_string());
            break;
        }
        pressed += 1;
    }

    if first_err.is_none() {
        if let Err(e) = enigo.key(primary, Direction::Click) {
            first_err = Some(e.to_string());
        }
    }

    for modifier in modifiers[..pressed].iter().rev() {
        if let Err(e) = enigo.key(*modifier, Direction::Release) {
            if first_err.is_none() {
                first_err = Some(e.to_string());
            }
        }
    }

    first_err.map_or(Ok(()), Err)
}

#[cfg(target_os = "windows")]
mod windows_shortcut {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY,
    };

    const VK_BACKSPACE: VIRTUAL_KEY = VIRTUAL_KEY(0x08);
    const VK_TAB: VIRTUAL_KEY = VIRTUAL_KEY(0x09);
    const VK_RETURN: VIRTUAL_KEY = VIRTUAL_KEY(0x0D);
    const VK_SHIFT: VIRTUAL_KEY = VIRTUAL_KEY(0x10);
    const VK_CONTROL: VIRTUAL_KEY = VIRTUAL_KEY(0x11);
    const VK_ALT: VIRTUAL_KEY = VIRTUAL_KEY(0x12);
    const VK_ESCAPE: VIRTUAL_KEY = VIRTUAL_KEY(0x1B);
    const VK_SPACE: VIRTUAL_KEY = VIRTUAL_KEY(0x20);
    const VK_PAGE_UP: VIRTUAL_KEY = VIRTUAL_KEY(0x21);
    const VK_PAGE_DOWN: VIRTUAL_KEY = VIRTUAL_KEY(0x22);
    const VK_END: VIRTUAL_KEY = VIRTUAL_KEY(0x23);
    const VK_HOME: VIRTUAL_KEY = VIRTUAL_KEY(0x24);
    const VK_LEFT: VIRTUAL_KEY = VIRTUAL_KEY(0x25);
    const VK_UP: VIRTUAL_KEY = VIRTUAL_KEY(0x26);
    const VK_RIGHT: VIRTUAL_KEY = VIRTUAL_KEY(0x27);
    const VK_DOWN: VIRTUAL_KEY = VIRTUAL_KEY(0x28);
    const VK_DELETE: VIRTUAL_KEY = VIRTUAL_KEY(0x2E);
    const VK_LEFT_WIN: VIRTUAL_KEY = VIRTUAL_KEY(0x5B);

    pub fn send(binding: &ShortcutBinding) -> Result<(), String> {
        let modifiers = modifier_vks(binding)?;
        let primary = primary_vk(&binding.primary)?;
        let mut inputs = Vec::with_capacity(modifiers.len() * 2 + 2);

        for modifier in &modifiers {
            inputs.push(keyboard_event(*modifier, false));
        }
        inputs.push(keyboard_event(primary, false));
        inputs.push(keyboard_event(primary, true));
        for modifier in modifiers.iter().rev() {
            inputs.push(keyboard_event(*modifier, true));
        }

        let sent = unsafe { SendInput(&mut inputs, std::mem::size_of::<INPUT>() as i32) };
        if (sent as usize) != inputs.len() {
            return Err(format!("SendInput sent {sent}/{}", inputs.len()));
        }
        Ok(())
    }

    fn modifier_vks(binding: &ShortcutBinding) -> Result<Vec<VIRTUAL_KEY>, String> {
        let mut keys = Vec::new();
        for raw in &binding.modifiers {
            let key = match raw.trim().to_ascii_lowercase().as_str() {
                "cmd" | "command" | "super" | "meta" | "win" => VK_LEFT_WIN,
                "ctrl" | "control" => VK_CONTROL,
                "alt" | "option" | "opt" => VK_ALT,
                "shift" => VK_SHIFT,
                other => return Err(format!("unsupported shortcut modifier: {other}")),
            };
            keys.push(key);
        }
        Ok(keys)
    }

    fn primary_vk(raw: &str) -> Result<VIRTUAL_KEY, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("shortcut primary is empty".into());
        }
        if trimmed.chars().count() == 1 {
            let ch = trimmed.chars().next().ok_or("single char expected")?;
            return char_vk(ch).ok_or_else(|| format!("unsupported shortcut primary: {trimmed}"));
        }

        let upper = trimmed.to_ascii_uppercase();
        let key = match upper.as_str() {
            "ENTER" | "RETURN" => VK_RETURN,
            "TAB" => VK_TAB,
            "ESC" | "ESCAPE" => VK_ESCAPE,
            "SPACE" => VK_SPACE,
            "BACKSPACE" => VK_BACKSPACE,
            "DELETE" | "DEL" => VK_DELETE,
            "HOME" => VK_HOME,
            "END" => VK_END,
            "PAGEUP" => VK_PAGE_UP,
            "PAGEDOWN" => VK_PAGE_DOWN,
            "ARROWUP" | "UP" => VK_UP,
            "ARROWDOWN" | "DOWN" => VK_DOWN,
            "ARROWLEFT" | "LEFT" => VK_LEFT,
            "ARROWRIGHT" | "RIGHT" => VK_RIGHT,
            value if function_key_vk(value).is_some() => function_key_vk(value).unwrap(),
            _ => return Err(format!("unsupported shortcut primary: {trimmed}")),
        };
        Ok(key)
    }

    fn char_vk(ch: char) -> Option<VIRTUAL_KEY> {
        match ch {
            'a'..='z' => Some(VIRTUAL_KEY(ch.to_ascii_uppercase() as u16)),
            'A'..='Z' | '0'..='9' => Some(VIRTUAL_KEY(ch as u16)),
            ' ' => Some(VK_SPACE),
            ';' => Some(VIRTUAL_KEY(0xBA)),
            '=' => Some(VIRTUAL_KEY(0xBB)),
            ',' => Some(VIRTUAL_KEY(0xBC)),
            '-' => Some(VIRTUAL_KEY(0xBD)),
            '.' => Some(VIRTUAL_KEY(0xBE)),
            '/' => Some(VIRTUAL_KEY(0xBF)),
            '`' => Some(VIRTUAL_KEY(0xC0)),
            '[' => Some(VIRTUAL_KEY(0xDB)),
            '\\' => Some(VIRTUAL_KEY(0xDC)),
            ']' => Some(VIRTUAL_KEY(0xDD)),
            '\'' => Some(VIRTUAL_KEY(0xDE)),
            _ => None,
        }
    }

    fn function_key_vk(value: &str) -> Option<VIRTUAL_KEY> {
        let number = value.strip_prefix('F')?.parse::<u16>().ok()?;
        if (1..=24).contains(&number) {
            Some(VIRTUAL_KEY(0x6F + number))
        } else {
            None
        }
    }

    fn keyboard_event(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        if key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_common_windows_shortcut_keys() {
            assert_eq!(char_vk('v').expect("v").0, 0x56);
            assert_eq!(char_vk('Z').expect("Z").0, 0x5A);
            assert_eq!(function_key_vk("F13").expect("F13").0, 0x7C);
            assert_eq!(function_key_vk("F24").expect("F24").0, 0x87);
        }
    }
}

fn shortcut_modifiers(binding: &ShortcutBinding) -> Result<Vec<enigo::Key>, String> {
    let mut keys = Vec::new();
    for raw in &binding.modifiers {
        let key = match raw.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "super" | "meta" | "win" => enigo::Key::Meta,
            "ctrl" | "control" => enigo::Key::Control,
            "alt" | "option" | "opt" => enigo::Key::Alt,
            "shift" => enigo::Key::Shift,
            other => return Err(format!("unsupported shortcut modifier: {other}")),
        };
        keys.push(key);
    }
    Ok(keys)
}

fn shortcut_primary(raw: &str) -> Result<enigo::Key, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("shortcut primary is empty".into());
    }
    if trimmed.chars().count() == 1 {
        let ch = trimmed.chars().next().ok_or("single char expected")?;
        return Ok(enigo::Key::Unicode(ch));
    }

    let upper = trimmed.to_ascii_uppercase();
    let key = match upper.as_str() {
        "ENTER" | "RETURN" => enigo::Key::Return,
        "TAB" => enigo::Key::Tab,
        "ESC" | "ESCAPE" => enigo::Key::Escape,
        "SPACE" => enigo::Key::Space,
        "BACKSPACE" => enigo::Key::Backspace,
        "DELETE" | "DEL" => enigo::Key::Delete,
        "HOME" => enigo::Key::Home,
        "END" => enigo::Key::End,
        "PAGEUP" => enigo::Key::PageUp,
        "PAGEDOWN" => enigo::Key::PageDown,
        "ARROWUP" | "UP" => enigo::Key::UpArrow,
        "ARROWDOWN" | "DOWN" => enigo::Key::DownArrow,
        "ARROWLEFT" | "LEFT" => enigo::Key::LeftArrow,
        "ARROWRIGHT" | "RIGHT" => enigo::Key::RightArrow,
        "F1" => enigo::Key::F1,
        "F2" => enigo::Key::F2,
        "F3" => enigo::Key::F3,
        "F4" => enigo::Key::F4,
        "F5" => enigo::Key::F5,
        "F6" => enigo::Key::F6,
        "F7" => enigo::Key::F7,
        "F8" => enigo::Key::F8,
        "F9" => enigo::Key::F9,
        "F10" => enigo::Key::F10,
        "F11" => enigo::Key::F11,
        "F12" => enigo::Key::F12,
        "F13" => enigo::Key::F13,
        "F14" => enigo::Key::F14,
        "F15" => enigo::Key::F15,
        "F16" => enigo::Key::F16,
        "F17" => enigo::Key::F17,
        "F18" => enigo::Key::F18,
        "F19" => enigo::Key::F19,
        "F20" => enigo::Key::F20,
        "F21" => enigo::Key::F21,
        "F22" => enigo::Key::F22,
        "F23" => enigo::Key::F23,
        "F24" => enigo::Key::F24,
        _ => return Err(format!("unsupported shortcut primary: {trimmed}")),
    };
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_fallback_function_keys() {
        assert_eq!(
            shortcut_primary("F13").expect("F13 parses"),
            enigo::Key::F13
        );
        assert_eq!(
            shortcut_primary("F16").expect("F16 parses"),
            enigo::Key::F16
        );
    }

    #[test]
    fn parses_common_shortcut_modifiers() {
        let binding = ShortcutBinding {
            primary: "K".into(),
            modifiers: vec!["ctrl".into(), "shift".into()],
        };
        let modifiers = shortcut_modifiers(&binding).expect("modifiers parse");
        assert_eq!(modifiers, vec![enigo::Key::Control, enigo::Key::Shift]);
        assert_eq!(
            shortcut_primary(&binding.primary).expect("primary parses"),
            enigo::Key::Unicode('K')
        );
    }
}
