use super::PersistenceError;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static AFTER_DELIVERY_QUARANTINE: Mutex<Option<PathBuf>> = Mutex::new(None);
static BEFORE_CURSOR_WRITE: Mutex<Option<PathBuf>> = Mutex::new(None);
static SECOND_CURSOR_WRITE: Mutex<Option<(PathBuf, usize)>> = Mutex::new(None);

pub(super) fn arm_before_cursor_write(path: PathBuf) {
    *BEFORE_CURSOR_WRITE.lock().expect("failpoint lock") = Some(path);
}

pub(super) fn before_cursor_write(path: &Path) -> Result<(), PersistenceError> {
    let mut armed = BEFORE_CURSOR_WRITE.lock().expect("failpoint lock");
    if armed.as_deref() == Some(path) {
        *armed = None;
        return Err(PersistenceError::Invalid(
            "test failpoint before cursor write".to_owned(),
        ));
    }
    let mut second = SECOND_CURSOR_WRITE.lock().expect("failpoint lock");
    if let Some((armed_path, remaining)) = second.as_mut() {
        if armed_path == path {
            if *remaining > 0 {
                *remaining -= 1;
                return Ok(());
            }
            *second = None;
            return Err(PersistenceError::Invalid(
                "test failpoint on redundant cursor write".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn arm_second_cursor_write(path: PathBuf) {
    *SECOND_CURSOR_WRITE.lock().expect("failpoint lock") = Some((path, 8));
}

pub(super) fn clear_second_cursor_write(path: &Path) {
    let mut armed = SECOND_CURSOR_WRITE.lock().expect("failpoint lock");
    if armed
        .as_ref()
        .is_some_and(|(armed_path, _)| armed_path == path)
    {
        *armed = None;
    }
}

pub(super) fn arm_after_delivery_quarantine(path: PathBuf) {
    *AFTER_DELIVERY_QUARANTINE.lock().expect("failpoint lock") = Some(path);
}

pub(super) fn after_delivery_quarantine(path: &Path) -> Result<(), PersistenceError> {
    let mut armed = AFTER_DELIVERY_QUARANTINE.lock().expect("failpoint lock");
    if armed.as_deref() == Some(path) {
        *armed = None;
        return Err(PersistenceError::Invalid(
            "test failpoint after delivery quarantine".to_owned(),
        ));
    }
    Ok(())
}
