use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmotionState {
    #[serde(deserialize_with = "level")]
    pub joy: u8,
    #[serde(deserialize_with = "level")]
    pub embarrassment: u8,
    #[serde(deserialize_with = "level")]
    pub concern: u8,
    #[serde(deserialize_with = "level")]
    pub surprise: u8,
    #[serde(deserialize_with = "level")]
    pub curiosity: u8,
    #[serde(deserialize_with = "level")]
    pub frustration: u8,
}

fn level<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let value = u8::deserialize(deserializer)?;
    if value > 100 {
        return Err(serde::de::Error::custom("emotion level exceeds 100"));
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct EmotionDelta {
    #[serde(deserialize_with = "delta")]
    pub joy: i8,
    #[serde(deserialize_with = "delta")]
    pub embarrassment: i8,
    #[serde(deserialize_with = "delta")]
    pub concern: i8,
    #[serde(deserialize_with = "delta")]
    pub surprise: i8,
    #[serde(deserialize_with = "delta")]
    pub curiosity: i8,
    #[serde(deserialize_with = "delta")]
    pub frustration: i8,
}

fn delta<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i8, D::Error> {
    let value = i8::deserialize(deserializer)?;
    if !(-100..=100).contains(&value) {
        return Err(serde::de::Error::custom("emotion delta is out of range"));
    }
    Ok(value)
}

pub(crate) fn optional_delta<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<EmotionDelta>, D::Error> {
    // 感情だけの不正出力で通常返信を失わない契約。
    let value = serde_json::Value::deserialize(deserializer)?;
    if !value.is_object() {
        return Ok(None);
    }
    Ok(serde_json::from_value(value).ok())
}

impl EmotionState {
    pub(crate) fn apply(self, delta: EmotionDelta) -> Self {
        fn add(value: u8, change: i8) -> u8 {
            (i16::from(value) + i16::from(change)).clamp(0, 100) as u8
        }
        Self {
            joy: add(self.joy, delta.joy),
            embarrassment: add(self.embarrassment, delta.embarrassment),
            concern: add(self.concern, delta.concern),
            surprise: add(self.surprise, delta.surprise),
            curiosity: add(self.curiosity, delta.curiosity),
            frustration: add(self.frustration, delta.frustration),
        }
    }
}

