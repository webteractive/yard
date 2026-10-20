//! Keeping descriptors out of the children we spawn.

use std::io;

const MAX_FDS: usize = 4096;

/// Marks every descriptor above stderr close-on-exec. Runs in the child between fork
/// and exec, so it makes only async-signal-safe calls.
///
/// macOS has no `SOCK_CLOEXEC`: a socket or pipe is created inheritable and marked
/// close-on-exec a moment later. A unit spawned in that moment by another thread would
/// keep, say, a port probe's socket and hold that port.
pub fn cloexec_inherited() -> io::Result<()> {
    let mut fds = [libc::proc_fdinfo { proc_fd: 0, proc_fdtype: 0 }; MAX_FDS];
    let size = std::mem::size_of_val(&fds) as libc::c_int;
    // SAFETY: the buffer is writable for `size` bytes; proc_pidinfo is a plain syscall.
    let n = unsafe {
        libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), size)
    };
    if n <= 0 {
        // Listing failed: spawning with a possible leak beats not starting the unit.
        return Ok(());
    }
    let count = n as usize / std::mem::size_of::<libc::proc_fdinfo>();
    for info in &fds[..count.min(MAX_FDS)] {
        if info.proc_fd > 2 {
            // SAFETY: fcntl on a descriptor the kernel just listed for this process.
            unsafe { libc::fcntl(info.proc_fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
    }
    Ok(())
}
