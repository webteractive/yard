mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;
use support::Fixture;
use tokio::sync::broadcast;
use yard_core::shim::child_path;
use yard_daemon::events::EventBus;
use yard_daemon::jobs::{JobCtx, Jobs};
use yard_proto::{ErrorCode, Event, YardError};

fn brew(fx: &Fixture) -> PathBuf {
    fx.brew.path().join("bin/brew")
}

/// Every event of job `id` until it finishes: its progress lines and its result.
async fn outcome(rx: &mut broadcast::Receiver<Event>, id: &str) -> (Vec<String>, Option<YardError>) {
    let mut lines = vec![];
    let wait = async {
        loop {
            match rx.recv().await.unwrap() {
                Event::JobProgress { job_id, line } if job_id == id => lines.push(line),
                Event::JobFinished { job_id, error, .. } if job_id == id => return error,
                _ => {}
            }
        }
    };
    let error = tokio::time::timeout(Duration::from_secs(10), wait).await.expect("the job finished");
    (lines, error)
}

#[tokio::test]
async fn job_streams_lines_then_finishes_ok() {
    let fx = Fixture::new();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let b = brew(&fx);
    let started = jobs.start("php-install", move |job: JobCtx| async move {
        job.run(&b, &["install", "shivammathur/php/php@7.1"], &[]).await
    });
    assert!(started.job_id.starts_with("php-install-"), "{}", started.job_id);
    let (lines, error) = outcome(&mut rx, &started.job_id).await;
    assert_eq!(error, None);
    assert!(lines.iter().any(|l| l.contains("fake brew install")), "{lines:?}");
    assert!(fx.brew.path().join("opt/php@7.1/sbin/php-fpm").exists());
}

#[tokio::test]
async fn failing_command_finishes_with_job_failed_and_tail() {
    let fx = Fixture::new();
    std::fs::write(fx.brew.path().join("fake-brew-fail"), "shivammathur/php/php@7.1\n").unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let b = brew(&fx);
    let started = jobs.start("php-install", move |job: JobCtx| async move {
        job.run(&b, &["install", "shivammathur/php/php@7.1"], &[]).await
    });
    let (_, error) = outcome(&mut rx, &started.job_id).await;
    let e = error.expect("a failure");
    assert_eq!(e.code, ErrorCode::JobFailed);
    assert!(e.message.contains("exited with"), "{}", e.message);
    let tail = e.data.as_ref().and_then(|d| d["tail"].as_array()).expect("a tail");
    assert!(!tail.is_empty());
}

#[tokio::test]
async fn relative_program_is_refused() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let started =
        jobs.start("x", |job: JobCtx| async move { job.run(&PathBuf::from("php"), &["-v"], &[]).await });
    let (_, error) = outcome(&mut rx, &started.job_id).await;
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::Internal));
}

#[tokio::test]
async fn job_path_puts_the_keg_first_and_drops_herd() {
    let fx = Fixture::new();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let keg_bin = fx.brew.path().join("opt/php@7.1/bin");
    let inherited = "/Users/x/Library/Application Support/Herd/bin/:/usr/bin";
    let path = child_path(Some(&keg_bin), inherited.as_ref()).to_string_lossy().into_owned();
    let fake = fx.fake.clone();
    let started = jobs.start("path", move |job: JobCtx| async move {
        job.run(&fake, &["print-path"], &[("PATH".to_string(), path)]).await
    });
    let (lines, error) = outcome(&mut rx, &started.job_id).await;
    assert_eq!(error, None);
    let printed = lines.iter().find(|l| l.starts_with("PATH=")).expect("the fake printed PATH");
    assert!(printed.starts_with(&format!("PATH={}", keg_bin.display())), "{printed}");
    assert!(!printed.contains("Herd"), "{printed}");
}

#[tokio::test]
async fn jobs_run_one_at_a_time() {
    let fx = Fixture::new();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let fake = fx.fake.clone();
    let first = jobs.start("a", move |job: JobCtx| async move {
        job.line("a starts");
        job.run(&fake, &["exit", "0", "300"], &[]).await
    });
    let second = jobs.start("b", |job: JobCtx| async move {
        job.line("b starts");
        Ok(())
    });
    let mut order = vec![];
    let wait = async {
        loop {
            match rx.recv().await.unwrap() {
                Event::JobProgress { job_id, line } => order.push(format!("{job_id}: {line}")),
                Event::JobFinished { job_id, .. } => {
                    order.push(format!("finished {job_id}"));
                    if job_id == second.job_id {
                        return;
                    }
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await.unwrap();
    let pos = |s: String| order.iter().position(|o| *o == s).unwrap();
    assert!(
        pos(format!("finished {}", first.job_id)) < pos(format!("{}: b starts", second.job_id)),
        "{order:?}"
    );
    // While it waits, the second says for which.
    assert_eq!(order[0], format!("{}: waiting for {}", second.job_id, first.job_id), "{order:?}");
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test]
async fn cancel_stops_a_running_command_and_its_children() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    // A shell that starts a child, as brew starts curl and make.
    let started = jobs.start("long", |job: JobCtx| async move {
        job.run(Path::new("/bin/sh"), &["-c", "sleep 30 & echo child $!; wait"], &[]).await?;
        job.line("after the command");
        Ok(())
    });
    let mut child = None;
    while child.is_none() {
        if let Ok(Event::JobProgress { line, .. }) = rx.recv().await {
            child = line.strip_prefix("child ").and_then(|p| p.parse::<i32>().ok());
        }
    }
    jobs.cancel(&started.job_id).unwrap();
    let (lines, error) = outcome(&mut rx, &started.job_id).await;
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::JobCancelled));
    assert!(!lines.iter().any(|l| l == "after the command"), "{lines:?}");
    let child = child.unwrap();
    let gone = || !alive(child);
    assert!(
        support::wait_until(gone, Duration::from_secs(5)).await,
        "the shell's child {child} was stopped too"
    );
}

#[tokio::test]
async fn a_cancelled_queued_job_never_runs() {
    let fx = Fixture::new();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let fake = fx.fake.clone();
    let first =
        jobs.start("a", move |job: JobCtx| async move { job.run(&fake, &["exit", "0", "300"], &[]).await });
    let second = jobs.start("b", |job: JobCtx| async move {
        job.line("b starts");
        Ok(())
    });
    jobs.cancel(&second.job_id).unwrap();
    // The cancelled job finishes at once, while the first still runs.
    let (lines, error) = outcome(&mut rx, &second.job_id).await;
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::JobCancelled));
    assert!(!lines.iter().any(|l| l == "b starts"), "{lines:?}");
    assert_eq!(outcome(&mut rx, &first.job_id).await.1, None, "the first ran on");
}

#[tokio::test]
async fn cancelling_an_unknown_or_finished_job_is_job_not_found() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    assert_eq!(jobs.cancel("nope-1").unwrap_err().code, ErrorCode::JobNotFound);
    let done = jobs.start("quick", |_job: JobCtx| async move { Ok(()) });
    assert_eq!(outcome(&mut rx, &done.job_id).await.1, None);
    let e = jobs.cancel(&done.job_id).unwrap_err();
    assert_eq!(e.code, ErrorCode::JobNotFound);
    assert!(e.message.contains(&done.job_id), "{}", e.message);
}

/// The `result` of job `id`'s `job_finished`.
async fn finished_result(rx: &mut broadcast::Receiver<Event>, id: &str) -> Option<serde_json::Value> {
    let wait = async {
        loop {
            if let Event::JobFinished { job_id, result, .. } = rx.recv().await.unwrap()
                && job_id == id
            {
                return result;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await.expect("the job finished")
}

#[tokio::test]
async fn a_job_result_rides_job_finished() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let ok = jobs.start("tunnel-add", |job: JobCtx| async move {
        job.set_result(serde_json::json!({"n": 0}));
        job.set_result(serde_json::json!({"n": 1}));
        Ok(())
    });
    assert_eq!(finished_result(&mut rx, &ok.job_id).await, Some(serde_json::json!({"n": 1})));
    let failed = jobs.start("tunnel-add", |job: JobCtx| async move {
        job.set_result(serde_json::json!({"n": 2}));
        Err(YardError::internal("it broke after setting a result"))
    });
    assert_eq!(finished_result(&mut rx, &failed.job_id).await, None);
}

#[tokio::test]
async fn a_queued_job_finishes_cancelled_at_once_not_when_the_one_ahead_ends() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let jobs = Jobs::new(bus.clone());
    let (release, held) = tokio::sync::oneshot::channel::<()>();
    let first = jobs.start("a", |_job: JobCtx| async move {
        let _ = held.await;
        Ok(())
    });
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_in_job = ran.clone();
    let second = jobs.start("b", move |_job: JobCtx| async move {
        ran_in_job.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    });

    jobs.cancel(&second.job_id).unwrap();

    // `a` still holds the queue; the cancelled `b` must not wait for it.
    let (_, error) = outcome(&mut rx, &second.job_id).await;
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::JobCancelled));
    release.send(()).unwrap();
    let (_, error) = outcome(&mut rx, &first.job_id).await;
    assert_eq!(error, None);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "a cancelled job never runs");
    assert!(jobs.cancel(&second.job_id).is_err(), "it is no longer queued");
}
