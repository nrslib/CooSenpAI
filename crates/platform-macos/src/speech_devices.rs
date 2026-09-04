use coosenpai_core::ports::{
    PortError, SpeechInputDevice, SpeechInputDevicePort, SpeechKeyStatePort,
};
use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID};

#[derive(Debug, Clone, Copy, Default)]
pub struct MacSpeechInputDevices;

impl SpeechInputDevicePort for MacSpeechInputDevices {
    #[allow(deprecated)]
    fn input_devices(&self) -> Result<Vec<SpeechInputDevice>, PortError> {
        // SAFETY: AVMediaTypeAudio is a process-lifetime framework constant. The returned array
        // retains every AVCaptureDevice while it is iterated.
        let devices = unsafe {
            let media_type = AVMediaTypeAudio.ok_or_else(|| {
                PortError::Unavailable("AVFoundation の音声 media type がありません".to_owned())
            })?;
            AVCaptureDevice::devicesWithMediaType(media_type)
        };
        let mut result = Vec::with_capacity(devices.len());
        for device in &devices {
            // SAFETY: These readonly properties return retained NSString values for a live device.
            let (id, name) = unsafe { (device.uniqueID(), device.localizedName()) };
            result.push(SpeechInputDevice {
                id: id.to_string(),
                name: name.to_string(),
            });
        }
        result.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(result)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MacSpeechKeyState;

impl SpeechKeyStatePort for MacSpeechKeyState {
    fn primary_key_pressed(&self, shortcut: &str) -> Result<bool, PortError> {
        let key = shortcut
            .split('+')
            .next_back()
            .map(str::trim)
            .and_then(key_code)
            .ok_or_else(|| {
                PortError::Unavailable(format!("push-to-talk の主キーを判別できません: {shortcut}"))
            })?;
        Ok(CGEventSource::key_state(
            CGEventSourceStateID::CombinedSessionState,
            key,
        ))
    }
}

fn key_code(key: &str) -> Option<u16> {
    let normalized = key.to_ascii_uppercase();
    Some(match normalized.as_str() {
        "BACKQUOTE" | "`" => 50,
        "BACKSLASH" | "\\" => 42,
        "BRACKETLEFT" | "[" => 33,
        "BRACKETRIGHT" | "]" => 30,
        "COMMA" | "," => 43,
        "EQUAL" | "=" => 24,
        "MINUS" | "-" => 27,
        "PERIOD" | "." => 47,
        "QUOTE" | "'" => 39,
        "SEMICOLON" | ";" => 41,
        "SLASH" | "/" => 44,
        "A" => 0,
        "S" => 1,
        "D" => 2,
        "F" => 3,
        "H" => 4,
        "G" => 5,
        "Z" => 6,
        "X" => 7,
        "C" => 8,
        "V" => 9,
        "B" => 11,
        "Q" => 12,
        "W" => 13,
        "E" => 14,
        "R" => 15,
        "Y" => 16,
        "T" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "9" => 25,
        "7" => 26,
        "8" => 28,
        "0" => 29,
        "O" => 31,
        "U" => 32,
        "I" => 34,
        "P" => 35,
        "L" => 37,
        "J" => 38,
        "K" => 40,
        "N" => 45,
        "M" => 46,
        "SPACE" => 49,
        "ENTER" | "RETURN" => 36,
        "TAB" => 48,
        "BACKSPACE" => 51,
        "CAPSLOCK" => 57,
        "F17" => 64,
        "NUMPADDECIMAL" | "NUMDECIMAL" => 65,
        "NUMPADMULTIPLY" | "NUMMULTIPLY" => 67,
        "NUMPADADD" | "NUMADD" | "NUMPADPLUS" | "NUMPLUS" => 69,
        "NUMLOCK" => 71,
        "PRINTSCREEN" => 70,
        "AUDIOVOLUMEUP" | "VOLUMEUP" => 72,
        "AUDIOVOLUMEDOWN" | "VOLUMEDOWN" => 73,
        "AUDIOVOLUMEMUTE" | "VOLUMEMUTE" => 74,
        "NUMPADDIVIDE" | "NUMDIVIDE" => 75,
        "NUMPADENTER" | "NUMENTER" => 76,
        "NUMPADSUBTRACT" | "NUMSUBTRACT" => 78,
        "F18" => 79,
        "F19" => 80,
        "NUMPADEQUAL" | "NUMEQUAL" => 81,
        "NUMPAD0" | "NUM0" => 82,
        "NUMPAD1" | "NUM1" => 83,
        "NUMPAD2" | "NUM2" => 84,
        "NUMPAD3" | "NUM3" => 85,
        "NUMPAD4" | "NUM4" => 86,
        "NUMPAD5" | "NUM5" => 87,
        "NUMPAD6" | "NUM6" => 88,
        "NUMPAD7" | "NUM7" => 89,
        "F20" => 90,
        "NUMPAD8" | "NUM8" => 91,
        "NUMPAD9" | "NUM9" => 92,
        "F5" => 96,
        "F6" => 97,
        "F7" => 98,
        "F3" => 99,
        "F8" => 100,
        "F9" => 101,
        "F11" => 103,
        "F13" => 105,
        "F16" => 106,
        "F14" => 107,
        "F10" => 109,
        "F12" => 111,
        "F15" => 113,
        "INSERT" => 114,
        "HOME" => 115,
        "PAGEUP" => 116,
        "DELETE" => 117,
        "F4" => 118,
        "END" => 119,
        "F2" => 120,
        "PAGEDOWN" => 121,
        "F1" => 122,
        "ARROWLEFT" | "LEFT" => 123,
        "ARROWRIGHT" | "RIGHT" => 124,
        "ARROWDOWN" | "DOWN" => 125,
        "ARROWUP" | "UP" => 126,
        _ => return None,
    })
}

