//! Which process holds a port, named so a conflict says "Herd" rather than a pid.
//!
//! Asks `lsof`, which reads the same libproc tables as the user: another user's process
//! (a root nginx master, say) is invisible and comes back as `None`.

use std::os::unix::process::CommandExt;
use std::process::Command;
use yard_core::ports::holder::classify;
use yard_proto::{Holder, HolderKind};

const LSOF: &str = "/usr/sbin/lsof";

pub fn find_holder(port: u16) -> Option<Holder> {
    let mut cmd = Command::new(LSOF);
    cmd.args(["-nP", "-w", "+c", "0", "-FpcR", "-sTCP:LISTEN"])
        .arg(format!("-iTCP:{port}"))
        .arg(format!("-iUDP:{port}"));
    // lsof must not keep a socket it inherited, which would hold that port while it runs.
    // SAFETY: the hook makes only async-signal-safe calls.
    unsafe { cmd.pre_exec(crate::fds::cloexec_inherited) };
    crate::fds::settle_before_fork();
    let out = cmd.output().ok()?;
    // lsof exits 1 when any one selector matched nothing, so read stdout regardless.
    let (pid, ppid, name) = parse_lsof(&String::from_utf8_lossy(&out.stdout))?;
    let path = pid_path(pid);
    let kind = classify(&name, path.as_deref(), ppid, None);
    Some(Holder { pid, name, path, ppid, kind })
}

/// How a conflict names the holder: `nginx-arm (pid 4534, Herd)`.
pub fn label(holder: Option<&Holder>) -> String {
    let Some(h) = holder else { return "a process not visible to this user".into() };
    let kind = match h.kind {
        HolderKind::Herd => "Herd",
        HolderKind::BrewServices => "brew services",
        HolderKind::Docker => "Docker",
        HolderKind::Dbngin => "DBngin",
        HolderKind::PostgresApp => "Postgres.app",
        HolderKind::Yard => "Yard",
        HolderKind::Unknown | HolderKind::Other => "unknown",
    };
    format!("{} (pid {}, {kind})", h.name, h.pid)
}

/// The first process in `lsof -F pcR` output: `p<pid>`, `R<ppid>`, `c<command>` lines.
fn parse_lsof(out: &str) -> Option<(u32, Option<u32>, String)> {
    let mut pid = None;
    let mut ppid = None;
    let mut name = String::new();
    for line in out.lines() {
        let (tag, value) = line.split_at_checked(1)?;
        match tag {
            "p" if pid.is_some() => break,
            "p" => pid = value.parse().ok(),
            "R" => ppid = value.parse().ok(),
            "c" => name = value.to_string(),
            _ => {}
        }
    }
    Some((pid?, ppid, name))
}

pub fn pid_path(pid: u32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the length passed, and proc_pidpath writes at most that.
    let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// The parent of `pid`, from the kernel's process table.
pub fn parent_of(pid: u32) -> Option<u32> {
    // SAFETY: zeroed is a valid proc_bsdinfo, which proc_pidinfo fills.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the buffer is writable for `size` bytes.
    let n =
        unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (n == size).then_some(info.pbi_ppid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn bound_port_holder_is_us() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        let h = find_holder(p).expect("our own listener is visible");
        assert_eq!(h.pid, std::process::id());
        assert!(h.path.is_some_and(|p| p.contains("yard_daemon")), "path is the test binary");
        assert_eq!(h.kind, HolderKind::Unknown);
    }

    #[test]
    fn free_port_has_no_holder() {
        let _ports = crate::PORT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let p = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(find_holder(p), None);
    }

    #[test]
    fn label_names_holder_kind_and_pid() {
        let herd = Holder {
            pid: 4534,
            name: "nginx-arm".into(),
            path: Some("/Applications/Herd.app/Contents/Resources/bin/nginx-arm".into()),
            ppid: Some(1),
            kind: HolderKind::Herd,
        };
        assert_eq!(label(Some(&herd)), "nginx-arm (pid 4534, Herd)");
        assert_eq!(label(None), "a process not visible to this user");
        let dbngin = Holder {
            pid: 77034,
            name: "mysqld".into(),
            path: Some("/Users/Shared/DBngin/mysql/8.0.19/bin/mysqld".into()),
            ppid: Some(1),
            kind: HolderKind::Dbngin,
        };
        assert_eq!(label(Some(&dbngin)), "mysqld (pid 77034, DBngin)");
        let pg = Holder { kind: HolderKind::PostgresApp, name: "postgres".into(), pid: 812, ..dbngin };
        assert_eq!(label(Some(&pg)), "postgres (pid 812, Postgres.app)");
    }

    #[test]
    fn parses_first_lsof_record() {
        let out = "p4534\nR4532\ncnginx-arm64\nf6\np4535\nR4532\ncnginx-arm64\nf6\n";
        assert_eq!(parse_lsof(out), Some((4534, Some(4532), "nginx-arm64".to_string())));
        assert_eq!(parse_lsof(""), None);
        assert_eq!(parse_lsof("p12\ncfoo\n"), Some((12, None, "foo".to_string())));
    }
}
