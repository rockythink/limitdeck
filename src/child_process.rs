//! Tracks private adapter process groups so a headless caller can cancel the whole collection.
#[cfg(unix)]
mod unix {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static CANCELLED: AtomicBool = AtomicBool::new(false);
    // The built-in discovery and fetch paths have at most three simultaneous children.
    static GROUPS: [AtomicI32; 16] = [const { AtomicI32::new(0) }; 16];

    extern "C" fn terminate(_: libc::c_int) {
        CANCELLED.store(true, Ordering::SeqCst);
        for group in &GROUPS {
            let pid = group.load(Ordering::SeqCst);
            if pid > 0 {
                // SAFETY: kill is async-signal-safe; every registered PID owns a private group.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }

    pub(super) fn stop() {
        terminate(0);
    }

    pub(super) fn install() {
        // Only the one-shot command installs handlers; the independent TUI is unchanged.
        unsafe {
            libc::signal(libc::SIGTERM, terminate as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, terminate as *const () as libc::sighandler_t);
        }
    }

    pub(super) fn track(pid: u32) {
        let Ok(pid) = i32::try_from(pid) else {
            return;
        };
        for group in &GROUPS {
            if group
                .compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }
        // Covers a cancellation arriving between spawn and registration.
        if CANCELLED.load(Ordering::SeqCst) {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }

    pub(super) fn forget(pid: u32) {
        for group in &GROUPS {
            let _ = group.compare_exchange(pid as i32, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    pub(super) fn cancelled() -> bool {
        CANCELLED.load(Ordering::SeqCst)
    }
}
pub(crate) fn stop_snapshot_children() {
    #[cfg(unix)]
    unix::stop();
}

pub(crate) fn install_snapshot_cancellation() {
    #[cfg(unix)]
    unix::install();
}

pub(crate) fn track(pid: u32) {
    #[cfg(unix)]
    unix::track(pid);
}

pub(crate) fn forget(pid: u32) {
    #[cfg(unix)]
    unix::forget(pid);
}

pub(crate) fn cancelled() -> bool {
    #[cfg(unix)]
    {
        unix::cancelled()
    }
    #[cfg(not(unix))]
    {
        false
    }
}
