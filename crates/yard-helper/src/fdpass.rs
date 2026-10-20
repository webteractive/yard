//! Sending descriptors over the helper socket with `SCM_RIGHTS`.

use std::io;
use std::os::fd::RawFd;

/// Sends `bytes` with `fds` attached to them in one `sendmsg`. Returns how many bytes
/// went: the descriptors travel with the first byte, so the caller writes any rest
/// without them. A non-blocking socket that is not writable fails with `WouldBlock`
/// and sends nothing.
pub fn send_with_fds(sock: RawFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let fd_bytes = std::mem::size_of_val(fds);
    let fd_len = u32::try_from(fd_bytes).map_err(|_| io::Error::other("too many descriptors"))?;
    // SAFETY: CMSG_SPACE is arithmetic on its argument.
    let space = unsafe { libc::CMSG_SPACE(fd_len) } as usize;
    // u64s keep the buffer aligned for cmsghdr.
    let mut control = vec![0u64; space.div_ceil(8)];
    let mut iov = libc::iovec { iov_base: bytes.as_ptr().cast_mut().cast(), iov_len: bytes.len() };
    // SAFETY: zeroed is a valid msghdr; the fields set below point at live buffers.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: the control buffer holds CMSG_SPACE(fd_len) bytes, so the first header and
    // its fd_len bytes of data fit; the descriptors are copied in, not borrowed.
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(fd_len) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr().cast::<u8>(), libc::CMSG_DATA(cmsg), fd_bytes);
    }
    // SAFETY: sendmsg reads the buffers msg points at, all alive for this call.
    let sent = unsafe { libc::sendmsg(sock, &msg, 0) };
    if sent < 0 { Err(io::Error::last_os_error()) } else { Ok(sent as usize) }
}
