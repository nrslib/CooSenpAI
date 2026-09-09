use base64::Engine;
use coosenpai_core::locale::{text, Locale, TextKey};
use minisign_verify::{PublicKey, Signature};

pub(crate) const MAX_ARCHIVE_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const BUNDLE_NAME: &str = "CooSenpAI.app";
pub(crate) const BUNDLE_IDENTIFIER: &str = "dev.nrslib.coosenpai";
pub(crate) const EXECUTABLE_NAME: &str = "coosenpai-desktop";

#[allow(dead_code)]
pub(crate) fn public_key(encoded: &str) -> Result<PublicKey, String> {
    public_key_for_locale(encoded, Locale::Ja)
}

pub(crate) fn public_key_for_locale(encoded: &str, locale: Locale) -> Result<PublicKey, String> {
    PublicKey::decode(&decode_for_locale(encoded, locale)?)
        .map_err(|_| text(TextKey::UpdatePublicKeyInvalid, locale).to_owned())
}

#[allow(dead_code)]
pub(crate) fn signature(encoded: &str) -> Result<Signature, String> {
    signature_for_locale(encoded, Locale::Ja)
}

pub(crate) fn signature_for_locale(encoded: &str, locale: Locale) -> Result<Signature, String> {
    Signature::decode(&decode_for_locale(encoded, locale)?)
        .map_err(|_| text(TextKey::UpdateSignatureInvalid, locale).to_owned())
}

#[allow(dead_code)]
fn decode(encoded: &str) -> Result<String, String> {
    decode_for_locale(encoded, Locale::Ja)
}

fn decode_for_locale(encoded: &str, locale: Locale) -> Result<String, String> {
    if encoded.len() > 16 * 1024 {
        return Err(text(TextKey::UpdateSignatureTooLarge, locale).to_owned());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| text(TextKey::UpdateSignatureEncodingInvalid, locale).to_owned())?;
    String::from_utf8(bytes)
        .map_err(|_| text(TextKey::UpdateSignatureUtf8Invalid, locale).to_owned())
}

pub(crate) struct VerifiedArchive(Vec<u8>);

impl VerifiedArchive {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

#[allow(dead_code)]
pub(crate) fn verify(
    bytes: Vec<u8>,
    signature: &Signature,
    key: &PublicKey,
) -> Result<VerifiedArchive, String> {
    verify_for_locale(bytes, signature, key, Locale::Ja)
}

pub(crate) fn verify_for_locale(
    bytes: Vec<u8>,
    signature: &Signature,
    key: &PublicKey,
    locale: Locale,
) -> Result<VerifiedArchive, String> {
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(text(TextKey::UpdateFileSizeInvalid, locale).to_owned());
    }
    key.verify(&bytes, signature, false)
        .map_err(|_| text(TextKey::UpdateSignatureVerificationFailed, locale).to_owned())?;
    Ok(VerifiedArchive(bytes))
}

