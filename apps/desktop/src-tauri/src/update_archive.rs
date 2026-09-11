use crate::update_format::{
    VerifiedArchive, BUNDLE_IDENTIFIER, BUNDLE_NAME, EXECUTABLE_NAME, MAX_ARCHIVE_BYTES,
};
use crate::update_system::{SystemVersion, UpdateError};
use coosenpai_core::locale::{text, Locale, TextKey};
use flate2::read::GzDecoder;
use semver::Version;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use tempfile::TempDir;

const MAX_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

pub(crate) struct StagedBundle {
    directory: TempDir,
    minimum_system_version: SystemVersion,
}

impl StagedBundle {
    pub(crate) fn path(&self) -> PathBuf {
        self.directory().join(BUNDLE_NAME)
    }

    pub(crate) fn directory(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn close(self) -> io::Result<()> {
        self.directory.close()
    }

    pub(crate) fn validate_minimum_system_version(
        &self,
        expected: SystemVersion,
        locale: Locale,
    ) -> Result<(), String> {
        if self.minimum_system_version != expected {
            return Err(text(TextKey::UpdateBundleMinimumMismatch, locale).to_owned());
        }
        Ok(())
    }

    pub(crate) fn ensure_system_supported(&self, system: SystemVersion) -> Result<(), UpdateError> {
        self.minimum_system_version.ensure_supported(system)
    }
}

#[allow(dead_code)]
pub(crate) fn prepare(
    archive: &VerifiedArchive,
    parent: &Path,
    expected: &Version,
    current: Option<&Version>,
    architecture: &str,
) -> Result<StagedBundle, String> {
    prepare_for_locale(archive, parent, expected, current, architecture, Locale::Ja)
}

pub(crate) fn prepare_for_locale(
    archive: &VerifiedArchive,
    parent: &Path,
    expected: &Version,
    current: Option<&Version>,
    architecture: &str,
    locale: Locale,
) -> Result<StagedBundle, String> {
    if !expected.pre.is_empty() || current.is_some_and(|current| expected <= current) {
        return Err(text(TextKey::UpdateVersionInvalid, locale).to_owned());
    }
    let directory = tempfile::Builder::new()
        .prefix(".coosenpai-update-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .map_err(|_| text(TextKey::UpdateWorkdirFailed, locale).to_owned())?;
    extract_for_locale(archive.bytes(), directory.path(), locale).map_err(|error| {
        text(TextKey::UpdateArchiveInvalid, locale).replace("{error}", &error.to_string())
    })?;
    let minimum_system_version = validate_bundle_for_locale(
        &directory.path().join(BUNDLE_NAME),
        expected,
        architecture,
        locale,
    )?;
    Ok(StagedBundle {
        directory,
        minimum_system_version,
    })
}

fn extract_for_locale(bytes: &[u8], destination: &Path, locale: Locale) -> io::Result<()> {
    let decoder = GzDecoder::new(bytes);
    let bounded = ExpandedReader {
        inner: decoder,
        remaining: MAX_EXPANDED_BYTES,
        locale,
    };
    let mut archive = tar::Archive::new(bounded);
    let mut paths = HashSet::new();
    let mut expanded = 0u64;
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(invalid(text(TextKey::UpdateEntryLimit, locale)));
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        validate_path_for_locale(&path, locale)?;
        if !paths.insert(path.clone()) {
            return Err(invalid(text(TextKey::UpdateEntryDuplicate, locale)));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(invalid(text(TextKey::UpdateSpecialFile, locale)));
        }
        let size = entry.size();
        if size > MAX_ARCHIVE_BYTES as u64 || size > MAX_EXPANDED_BYTES.saturating_sub(expanded) {
            return Err(invalid(text(TextKey::UpdateExpandedSizeLimit, locale)));
        }
        expanded += size;
        let target = destination.join(&path);
        if kind.is_dir() {
            if size != 0 {
                return Err(invalid(text(TextKey::UpdateDirectoryData, locale)));
            }
            fs::create_dir_all(&target)?;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&target)?;
            let copied = io::copy(&mut entry, &mut file)?;
            if copied != size {
                return Err(invalid(text(TextKey::UpdateEntryLengthMismatch, locale)));
            }
            let executable = entry.header().mode()? & 0o111 != 0;
            file.set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o644
            }))?;
            file.sync_all()?;
        }
    }
    // 終端後も gzip trailer と展開総量を検証する。
    io::copy(&mut archive.into_inner(), &mut io::sink())?;
    Ok(())
}

fn validate_path_for_locale(path: &Path, locale: Locale) -> io::Result<()> {
    if path.as_os_str().len() > 4096 {
        return Err(invalid(text(TextKey::UpdateEntryPathTooLong, locale)));
    }
    let mut components = path.components();
    if components.next() != Some(Component::Normal(BUNDLE_NAME.as_ref()))
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid(text(TextKey::UpdatePathOutsideBundle, locale)));
    }
    Ok(())
}

#[cfg(test)]
fn validate_bundle(
    bundle: &Path,
    expected: &Version,
    architecture: &str,
) -> Result<SystemVersion, String> {
    validate_bundle_for_locale(bundle, expected, architecture, Locale::Ja)
}

fn validate_bundle_for_locale(
    bundle: &Path,
    expected: &Version,
    architecture: &str,
    locale: Locale,
) -> Result<SystemVersion, String> {
    let info = bundle.join("Contents/Info.plist");
    let metadata = fs::symlink_metadata(&info)
        .map_err(|_| text(TextKey::UpdateInfoPlistMissing, locale).to_owned())?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(text(TextKey::UpdateInfoPlistInvalid, locale).to_owned());
    }
    let value = plist::Value::from_file(info)
        .map_err(|_| text(TextKey::UpdateInfoPlistParseFailed, locale).to_owned())?;
    let dictionary = value
        .as_dictionary()
        .ok_or_else(|| text(TextKey::UpdateInfoPlistNotDictionary, locale).to_owned())?;
    let get = |key: &str| dictionary.get(key).and_then(plist::Value::as_string);
    if get("CFBundleIdentifier") != Some(BUNDLE_IDENTIFIER)
        || get("CFBundleExecutable") != Some(EXECUTABLE_NAME)
    {
        return Err(text(TextKey::UpdateBundleIdentityMismatch, locale).to_owned());
    }
    let version = get("CFBundleShortVersionString")
        .ok_or_else(|| text(TextKey::UpdateBundleVersionMissing, locale).to_owned())?;
    if Version::parse(version)
        .map_err(|_| text(TextKey::UpdateBundleVersionInvalid, locale).to_owned())?
        != *expected
    {
        return Err(text(TextKey::UpdateBundleVersionMismatch, locale).to_owned());
    }
    let executable = bundle.join("Contents/MacOS").join(EXECUTABLE_NAME);
    let metadata = fs::symlink_metadata(&executable)
        .map_err(|_| text(TextKey::UpdateExecutableMissing, locale).to_owned())?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(text(TextKey::UpdateExecutableInvalid, locale).to_owned());
    }
    validate_macho_for_locale(&executable, architecture, locale)?;
    get("LSMinimumSystemVersion")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| text(TextKey::UpdateBundleMinimumInvalid, locale).to_owned())
}

fn validate_macho_for_locale(
    executable: &Path,
    architecture: &str,
    locale: Locale,
) -> Result<(), String> {
    const MH_MAGIC_64: u32 = 0xfeedfacf;
    const MH_EXECUTE: u32 = 2;
    const CPU_TYPE_ARM64: u32 = 0x0100000c;
    const CPU_TYPE_X86_64: u32 = 0x01000007;
    let expected_cpu = match architecture {
        "aarch64" => CPU_TYPE_ARM64,
        "x86_64" => CPU_TYPE_X86_64,
        _ => return Err(text(TextKey::UpdateArchitectureUnsupported, locale).to_owned()),
    };
    let mut header = [0u8; 32];
    fs::File::open(executable)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|_| text(TextKey::UpdateMachHeaderReadFailed, locale).to_owned())?;
    if header[..4] != MH_MAGIC_64.to_le_bytes() || header[12..16] != MH_EXECUTE.to_le_bytes() {
        return Err(text(TextKey::UpdateMachFormatInvalid, locale).to_owned());
    }
    if header[4..8] != expected_cpu.to_le_bytes() {
        return Err(text(TextKey::UpdateArchitectureMismatch, locale).to_owned());
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct ExpandedReader<R> {
    inner: R,
    remaining: u64,
    locale: Locale,
}

impl<R: Read> Read for ExpandedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let limit = buffer.len().min(self.remaining.saturating_add(1) as usize);
        let count = self.inner.read(&mut buffer[..limit])?;
        if count as u64 > self.remaining {
            return Err(invalid(text(
                TextKey::UpdateExpandedTotalLimit,
                self.locale,
            )));
        }
        self.remaining -= count as u64;
        Ok(count)
    }
}

