//! Receiving descriptors from the helper with `SCM_RIGHTS`.

use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// More than `ports.bind` ever sends; a message with more is refused.
const MAX_FDS: usize = 8;

/// Reads from a blocking socket up to and including the first newline, collecting every
/// descriptor that arrives with it. Each one is marked close-on-exec as it arrives:
/// macOS has no `MSG_CMSG_CLOEXEC`, and a unit spawned meanwhile must not inherit a
/// listener it was not given.
pub fn recv_line_with_fds(sock: RawFd) -> io::Result<(String, Vec<OwnedFd>)> {
    let mut line = Vec::new();
    let mut fds = Vec::new();
    let mut buf = [0u8; 4096];
    // SAFETY: CMSG_SPACE is arithmetic on its argument.
    let space = unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) } as usize;
    // u64s keep the buffer aligned for cmsghdr.
    let mut control = vec![0u64; space.div_ceil(8)];
    while !line.contains(&b'\n') {
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
        // SAFETY: zeroed is a valid msghdr; the fields set below point at live buffers.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        // SAFETY: recvmsg writes only into the buffers msg points at.
        let n = unsafe { libc::recvmsg(sock, &mut msg, 0) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        // Own what arrived before any early return, so nothing leaks.
        // SAFETY: the kernel filled msg's control buffer; CMSG_* walk only within it.
        unsafe { take_fds(&msg, &mut fds) };
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::other("the helper sent more descriptors than expected"));
        }
        if n == 0 {
            break;
        }
        line.extend_from_slice(&buf[..n as usize]);
    }
    let end = line.iter().position(|b| *b == b'\n').map_or(line.len(), |i| i + 1);
    Ok((String::from_utf8_lossy(&line[..end]).into_owned(), fds))
}

/// # Safety
/// `msg` must be the header of a completed `recvmsg`.
unsafe fn take_fds(msg: &libc::msghdr, fds: &mut Vec<OwnedFd>) {
    // SAFETY: per the contract, CMSG_FIRSTHDR/NXTHDR stay inside msg's control buffer.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg);
                let len = (*cmsg).cmsg_len as usize - (data as usize - cmsg as usize);
                for i in 0..len / std::mem::size_of::<RawFd>() {
                    let fd = std::ptr::read_unaligned(data.cast::<RawFd>().add(i));
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    fds.push(OwnedFd::from_raw_fd(fd));
                }
            }
            cmsg = libc::CMSG_NXTHDR(msg, cmsg);
        }
    }
}
