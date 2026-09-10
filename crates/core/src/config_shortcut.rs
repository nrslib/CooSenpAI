pub fn shortcut_identity(value: &str) -> Option<String> {
    let mut modifiers = Vec::new();
    let mut key = None;
    let tokens = value.split('+').collect::<Vec<_>>();
    let multiple = tokens.len() > 1;
    for raw in tokens {
        let part = if multiple { raw.trim() } else { raw };
        if part.is_empty() {
            return None;
        }
        let lower = part.to_ascii_lowercase();
        let modifier = match lower.as_str() {
            "shift" => Some("shift"),
            "alt" | "option" => Some("alt"),
            "control" | "ctrl" => Some("control"),
            "super" | "command" | "cmd" => Some("super"),
            "commandorcontrol" | "commandorctrl" | "cmdorcontrol" | "cmdorctrl" => Some("super"),
            _ => None,
        };
        if let Some(modifier) = modifier {
            if key.is_some() {
                return None;
            }
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
            continue;
        }
        if key.replace(normalize_key(&lower)?).is_some() {
            return None;
        }
    }
    let key = key?;
    modifiers.sort_unstable();
    Some(if modifiers.is_empty() {
        key
    } else {
        format!("{}+{key}", modifiers.join("+"))
    })
}

/// macOS の global-hotkey 0.8 が実際に登録できる主キーかを判定する。
///
/// parser が受理するキー集合には、他のプラットフォーム専用キーや macOS で
/// scan code に変換できない F21 以降も含まれるため、設定境界ではこの集合を正本にする。
pub fn is_macos_shortcut_key(identity: &str) -> bool {
    let key = identity.rsplit('+').next().unwrap_or(identity);
    if key.len() == 1 && key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return true;
    }
    if let Some(number) = key
        .strip_prefix('f')
        .and_then(|value| value.parse::<u8>().ok())
    {
        return (1..=20).contains(&number) && key == format!("f{number}");
    }
    matches!(
        key,
        "backquote"
            | "backslash"
            | "bracketleft"
            | "bracketright"
            | "comma"
            | "equal"
            | "minus"
            | "period"
            | "quote"
            | "semicolon"
            | "slash"
            | "backspace"
            | "capslock"
            | "enter"
            | "space"
            | "tab"
            | "delete"
            | "end"
            | "home"
            | "insert"
            | "pagedown"
            | "pageup"
            | "printscreen"
            | "arrowdown"
            | "arrowleft"
            | "arrowright"
            | "arrowup"
            | "numlock"
            | "numpad0"
            | "numpad1"
            | "numpad2"
            | "numpad3"
            | "numpad4"
            | "numpad5"
            | "numpad6"
            | "numpad7"
            | "numpad8"
            | "numpad9"
            | "numpadadd"
            | "numpaddecimal"
            | "numpaddivide"
            | "numpadenter"
            | "numpadequal"
            | "numpadmultiply"
            | "numpadsubtract"
            | "audiovolumedown"
            | "audiovolumeup"
            | "audiovolumemute"
            | "mediaplaypause"
            | "mediatracknext"
            | "mediatrackprevious"
    )
}

fn normalize_key(key: &str) -> Option<String> {
    if let Some(character) = key
        .strip_prefix("key")
        .filter(|value| value.len() == 1 && value.bytes().all(|byte| byte.is_ascii_alphabetic()))
    {
        return Some(character.to_owned());
    }
    if let Some(digit) = key
        .strip_prefix("digit")
        .filter(|value| value.len() == 1 && value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Some(digit.to_owned());
    }
    if key.len() == 1 && key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Some(key.to_owned());
    }
    Some(
        match key {
            "`" => "backquote",
            "\\" => "backslash",
            "[" => "bracketleft",
            "]" => "bracketright",
            "pausebreak" => "pause",
            "," => "comma",
            "=" => "equal",
            "-" => "minus",
            "." => "period",
            "'" => "quote",
            ";" => "semicolon",
            "/" => "slash",
            "down" => "arrowdown",
            "left" => "arrowleft",
            "right" => "arrowright",
            "up" => "arrowup",
            "esc" => "escape",
            "volumedown" => "audiovolumedown",
            "volumeup" => "audiovolumeup",
            "volumemute" => "audiovolumemute",
            "mediatrackprev" => "mediatrackprevious",
            value => return normalize_named_key(value),
        }
        .to_owned(),
    )
}

fn normalize_named_key(key: &str) -> Option<String> {
    let canonical = match key {
        "backquote" | "backslash" | "bracketleft" | "bracketright" | "pause" | "comma"
        | "equal" | "minus" | "period" | "quote" | "semicolon" | "slash" | "backspace"
        | "capslock" | "enter" | "space" | "tab" | "delete" | "end" | "home" | "insert"
        | "pagedown" | "pageup" | "printscreen" | "scrolllock" | "arrowdown" | "arrowleft"
        | "arrowright" | "arrowup" | "numlock" | "escape" | "audiovolumedown" | "audiovolumeup"
        | "audiovolumemute" | "mediaplay" | "mediapause" | "mediaplaypause" | "mediastop"
        | "mediatracknext" | "mediatrackprevious" => return Some(key.to_owned()),
        _ => key,
    };
    if let Some(number) = canonical
        .strip_prefix('f')
        .and_then(|value| value.parse::<u8>().ok())
    {
        let normalized = format!("f{number}");
        return ((1..=24).contains(&number) && canonical == normalized).then_some(normalized);
    }
    let suffix = canonical
        .strip_prefix("numpad")
        .or_else(|| canonical.strip_prefix("num"))?;
    Some(match suffix {
        value if value.len() == 1 && value.bytes().all(|byte| byte.is_ascii_digit()) => {
            format!("numpad{value}")
        }
        "add" | "plus" => "numpadadd".to_owned(),
        "decimal" => "numpaddecimal".to_owned(),
        "divide" => "numpaddivide".to_owned(),
        "enter" => "numpadenter".to_owned(),
        "equal" => "numpadequal".to_owned(),
        "multiply" => "numpadmultiply".to_owned(),
        "subtract" => "numpadsubtract".to_owned(),
        _ => return None,
    })
}

