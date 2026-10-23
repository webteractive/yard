//! Leftover service processes: a yardd that was killed leaves its units running, holding
//! their ports and data directories (spec §15). Each unit with a pid file records its
//! process there, so the next yardd can stop it before starting a second one.

use std::path::Path;
use std::time::Duration;
use tokio::time::Instant;

/// How often a stop polls for the process to be gone.
const POLL: Duration = Duration::from_millis(25);
/// How long SIGKILL gets to take effect, so a returned pid means the port is free.
const KILL_WAIT: Duration = Duration::from_secs(2);

/// Stops the process `pid_file` names when it is `program` and launchd is its parent
/// (an orphan of an earlier yardd): `signal`, the unit's own stop signal (PostgreSQL's
/// SIGINT is its fast shutdown, where SIGTERM waits for clients), then SIGKILL after
/// `grace`. Removes the file and returns the pid it stopped.
///
/// Anything else returns `None`. A file that is unreadable, names a dead process or names
/// a different program is stale and is removed. A live `program` with a parent other than
/// launchd is somebody's supervised unit, so it and its file are left alone. Waits with
/// tokio's clock, so it never blocks a runtime thread.
pub async fn stop(pid_file: &Path, program: &Path, grace: Duration, signal: i32) -> Option<u32> {
    let Some(pid) = read_pid(pid_file) else {
        remove(pid_file);
        return None;
    };
    let ours = std::fs::canonicalize(program)
        .ok()
        .zip(crate::holder::pid_path(pid))
        .is_some_and(|(program, path)| Path::new(&path) == program);
    if !ours {
        remove(pid_file);
        return None;
    }
    if crate::holder::parent_of(pid) != Some(1) {
        return None;
    }
    terminate(pid, grace, signal).await;
    remove(pid_file);
    Some(pid)
}

fn read_pid(pid_file: &Path) -> Option<u32> {
    // pid 0 and 1 would signal a process group or launchd itself.
    std::fs::read_to_string(pid_file).ok()?.trim().parse().ok().filter(|pid| *pid > 1)
}

fn remove(pid_file: &Path) {
    let _ = std::fs::remove_file(pid_file);
}

async fn terminate(pid: u32, grace: Duration, signal: i32) {
    let Ok(raw) = i32::try_from(pid) else { return };
    // SAFETY: kill(2) on a pid just checked to be an orphaned unit of ours.
    unsafe { libc::kill(raw, signal) };
    if !gone_within(raw, grace).await {
        // SAFETY: as above.
        unsafe { libc::kill(raw, libc::SIGKILL) };
        gone_within(raw, KILL_WAIT).await;
    }
}

async fn gone_within(pid: i32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        // SAFETY: signal 0 only checks that the pid exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}
