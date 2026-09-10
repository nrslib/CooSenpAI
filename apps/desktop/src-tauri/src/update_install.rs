use crate::update_archive::StagedBundle;
use crate::update_format::{BUNDLE_NAME, EXECUTABLE_NAME};
use crate::update_system::{SystemVersion, UpdateError};
use coosenpai_core::locale::{text, Locale, TextKey};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub(crate) struct InstallLocation {
    parent: PathBuf,
    parent_fd: File,
    device: u64,
    inode: u64,
    locale: Locale,
}

impl InstallLocation {
    pub(crate) fn for_running_app_for_locale(
        executable: &Path,
        home: &Path,
        locale: Locale,
    ) -> Result<Self, String> {
        let executable = fs::canonicalize(executable).map_err(|error| error.to_string())?;
        let bundle = executable
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or_else(|| text(TextKey::UpdateAppLocationInvalid, locale).to_owned())?;
        if executable != bundle.join("Contents/MacOS").join(EXECUTABLE_NAME) {
            return Err(text(TextKey::UpdateAppExecutableLocationInvalid, locale).to_owned());
        }
        let parent = bundle
            .parent()
            .ok_or_else(|| text(TextKey::UpdateAppLocationInvalid, locale).to_owned())?;
        let supported = [PathBuf::from("/Applications"), home.join("Applications")];
        if bundle.file_name() != Some(BUNDLE_NAME.as_ref())
            || !supported
                .iter()
                .any(|path| fs::canonicalize(path).is_ok_and(|path| path == parent))
        {
            return Err(text(TextKey::UpdateAppMustBeInApplications, locale).to_owned());
        }
        Self::open_for_locale(parent, locale)
            .map_err(|_| text(TextKey::UpdateAppLocationOpenFailed, locale).to_owned())
    }

    fn open_for_locale(parent: &Path, locale: Locale) -> io::Result<Self> {
        let parent_fd = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(parent)?;
        let parent_metadata = parent_fd.metadata()?;
        // 他ユーザーが置換可能な配置先は使わない。root:admin の /Applications は許可する。
        let trusted_group = parent_metadata.uid() == 0 && parent_metadata.gid() == 80;
        if ![0, unsafe { libc::geteuid() }].contains(&parent_metadata.uid())
            || parent_metadata.mode() & 0o002 != 0
            || (parent_metadata.mode() & 0o020 != 0 && !trusted_group)
        {
            return Err(io::Error::other(text(
                TextKey::UpdateInstallLocationUnsafe,
                locale,
            )));
        }
        let metadata = fs::symlink_metadata(parent.join(BUNDLE_NAME))?;
        if !metadata.is_dir() {
            return Err(io::Error::other(text(
                TextKey::UpdateTargetNotDirectory,
                locale,
            )));
        }
        Ok(Self {
            parent: parent.to_owned(),
            parent_fd,
            device: metadata.dev(),
            inode: metadata.ino(),
            locale,
        })
    }

    pub(crate) fn parent(&self) -> &Path {
        &self.parent
    }

    pub(crate) fn apply(
        &self,
        staged: &StagedBundle,
        minimum: SystemVersion,
        system: SystemVersion,
    ) -> Result<(), UpdateError> {
        staged.validate_minimum_system_version(minimum, self.locale)?;
        minimum.ensure_supported(system)?;
        self.swap(staged).map_err(|error| {
            UpdateError::Failed(
                text(TextKey::UpdatePreviousPreserved, self.locale)
                    .replace("{error}", &error.to_string()),
            )
        })
    }

    fn swap(&self, staged: &StagedBundle) -> io::Result<()> {
        let parent_metadata = self.parent_fd.metadata()?;
        let current_parent = fs::symlink_metadata(&self.parent)?;
        let current = fs::symlink_metadata(self.parent.join(BUNDLE_NAME))?;
        let new = fs::symlink_metadata(staged.path())?;
        if !current_parent.is_dir()
            || parent_metadata.dev() != current_parent.dev()
            || parent_metadata.ino() != current_parent.ino()
            || !current.is_dir()
            || current.dev() != self.device
            || current.ino() != self.inode
            || !new.is_dir()
        {
            return Err(io::Error::other(text(
                TextKey::UpdateTargetChanged,
                self.locale,
            )));
        }
        let stage_fd = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(staged.directory())?;
        atomic_swap(&self.parent_fd, &stage_fd)
    }
}

fn atomic_swap(parent: &File, stage: &File) -> io::Result<()> {
    let name = CString::new(BUNDLE_NAME).expect("static bundle name");
    // 両ディレクトリを FD に固定し、失敗時は旧版を変更しない同一 FS の交換だけを使う。
    let result = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            name.as_ptr(),
            stage.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

