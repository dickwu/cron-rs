use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};

/// An flock-based lock guard for task concurrency control.
/// The lock is released when this guard is dropped.
pub struct LockGuard {
    _path: PathBuf,
    fd: RawFd,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Release the flock and close the fd
        unsafe {
            libc::flock(self.fd, libc::LOCK_UN);
            libc::close(self.fd);
        }
    }
}

/// Ensure the lock directory exists.
fn ensure_lock_dir(lock_dir: &Path) -> anyhow::Result<()> {
    if !lock_dir.exists() {
        std::fs::create_dir_all(lock_dir)?;
    }
    Ok(())
}

/// Build the lock file path for a task.
fn lock_path(lock_dir: &Path, task_id: &str) -> PathBuf {
    lock_dir.join(format!("{}.lock", task_id))
}

/// Open or create the lock file and return its raw fd.
fn open_lock_file(path: &Path) -> anyhow::Result<RawFd> {
    use std::ffi::CString;

    let path_cstr = CString::new(
        path.to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid lock path"))?,
    )?;

    let fd = unsafe {
        libc::open(
            path_cstr.as_ptr(),
            libc::O_CREAT | libc::O_RDWR,
            0o644,
        )
    };

    if fd < 0 {
        return Err(anyhow::anyhow!(
            "Failed to open lock file {:?}: {}",
            path,
            std::io::Error::last_os_error()
        ));
    }

    Ok(fd)
}

/// Acquire an flock-based lock for a task (blocking).
/// This will block until the lock is available.
pub fn acquire_lock(lock_dir: &Path, task_id: &str) -> anyhow::Result<LockGuard> {
    ensure_lock_dir(lock_dir)?;
    let path = lock_path(lock_dir, task_id);
    let fd = open_lock_file(&path)?;

    let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(anyhow::anyhow!(
            "Failed to acquire lock for task {}: {}",
            task_id,
            err
        ));
    }

    Ok(LockGuard { _path: path, fd })
}

/// Try to acquire a lock without blocking. Returns None if the lock is held
/// by another process.
pub fn try_acquire_lock(lock_dir: &Path, task_id: &str) -> anyhow::Result<Option<LockGuard>> {
    ensure_lock_dir(lock_dir)?;
    let path = lock_path(lock_dir, task_id);
    let fd = open_lock_file(&path)?;

    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };

        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            // Lock is held by another process
            return Ok(None);
        }

        return Err(anyhow::anyhow!(
            "Failed to try-acquire lock for task {}: {}",
            task_id,
            err
        ));
    }

    Ok(Some(LockGuard { _path: path, fd }))
}

/// Check if a lock is currently held by another process.
/// Returns true if the lock is held (i.e., a non-blocking acquire would fail).
#[allow(dead_code)]
pub fn is_lock_held(lock_dir: &Path, task_id: &str) -> bool {
    match try_acquire_lock(lock_dir, task_id) {
        Ok(Some(_guard)) => {
            // We acquired it, so it wasn't held. The guard drops and releases.
            false
        }
        Ok(None) => true,
        Err(_) => false,
    }
}
