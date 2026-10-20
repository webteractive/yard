mod support;

use std::path::PathBuf;
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
                Event::JobFinished { job_id, error } if job_id == id => return error,
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
                Event::JobProgress { job_id, .. } => order.push(format!("progress {job_id}")),
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
        pos(format!("finished {}", first.job_id)) < pos(format!("progress {}", second.job_id)),
        "{order:?}"
    );
}
