use coosenpai_core::locale::{text, Locale, TextKey};
use serde::{Deserialize, Deserializer};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SystemVersion([u32; 3]);

impl FromStr for SystemVersion {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = "invalid macOS version";
        let mut components = [0; 3];
        let mut count = 0;
        for (index, part) in value.split('.').enumerate() {
            if index >= 3
                || part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return Err(invalid);
            }
            components[index] = part.parse().map_err(|_| invalid)?;
            count += 1;
        }
        if count < 2 || components[0] == 0 {
            return Err(invalid);
        }
        Ok(Self(components))
    }
}

impl<'de> Deserialize<'de> for SystemVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for SystemVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.0;
        write!(formatter, "{major}.{minor}")?;
        if patch != 0 {
            write!(formatter, ".{patch}")?;
        }
        Ok(())
    }
}

impl SystemVersion {
    pub(crate) fn current(locale: Locale) -> Result<Self, String> {
        let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
        format!(
            "{}.{}.{}",
            version.majorVersion, version.minorVersion, version.patchVersion
        )
        .parse()
        .map_err(|_| text(TextKey::UpdateSystemVersionUnavailable, locale).to_owned())
    }

    pub(crate) fn ensure_supported(self, current: Self) -> Result<(), UpdateError> {
        if current < self {
            return Err(UpdateError::IncompatibleSystem(self));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UpdateError {
    #[error("macOS {0} or later is required")]
    IncompatibleSystem(SystemVersion),
    #[error("{0}")]
    Failed(String),
}

impl From<String> for UpdateError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

