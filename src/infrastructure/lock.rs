use std::{fs, path::Path};

use fs2::FileExt;

use crate::TrackerError;

pub struct WriterLock {
    file: fs::File,
}

impl WriterLock {
    pub fn acquire(data_root: &Path) -> Result<Self, TrackerError> {
        fs::create_dir_all(data_root)
            .map_err(|_| TrackerError::storage("could not create application data directory"))?;
        super::registry::set_private_dir(data_root)?;
        let path = data_root.join(".write.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|_| TrackerError::storage("could not open writer lock"))?;
        super::registry::set_private_file(&path)?;
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                TrackerError::new(
                    "operation_in_progress",
                    "another state-changing operation is in progress",
                )
            } else {
                TrackerError::storage("could not acquire writer lock")
            }
        })?;
        Ok(Self { file })
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_writer_fails_without_waiting() {
        let temp = tempfile::tempdir().unwrap();
        let _first = WriterLock::acquire(temp.path()).unwrap();
        let second = match WriterLock::acquire(temp.path()) {
            Ok(_) => panic!("second writer unexpectedly acquired the lock"),
            Err(error) => error,
        };
        assert_eq!(second.code, "operation_in_progress");
    }
}
