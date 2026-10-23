//! `yard sources`, `yard source trust|untrust`, and the y/N trust prompt every install shares.

use super::{Ctx, Outcome, confirm, finished, interactive, run_job};
use crate::client::CliError;
use crate::output::{for_agents, table};
use serde_json::Value;
use std::future::Future;
use yard_proto::methods::{self, SourceParams};
use yard_proto::{ErrorCode, SourceInfo, YardError};

pub async fn list(ctx: &Ctx) -> Outcome {
    let sources: Vec<SourceInfo> = ctx.client().await?.call(methods::SOURCES_LIST, Value::Null).await?;
    ctx.emit(&sources, || human(&sources));
    Ok(0)
}

fn human(sources: &[SourceInfo]) -> String {
    let yes_no = |b: bool| if b { "yes" } else { "no" }.to_string();
    let mut rows: Vec<Vec<String>> = vec![];
    for source in sources {
        rows.push(vec![source.tap.clone(), yes_no(source.tapped), yes_no(source.trusted), "-".into()]);
        for f in &source.formulae {
            let installed = if f.installed { "yes" } else { "no" };
            rows.push(vec![format!("  {}", f.name), "-".into(), yes_no(f.trusted), installed.into()]);
        }
    }
    let mut out = table(&["SOURCE", "TAPPED", "TRUSTED", "INSTALLED"], &rows);
    out.push('\n');
    out.push_str("Trust one formula with yard source trust <formula>, or a whole tap with --tap.\n");
    out.push_str(&for_agents("yard sources"));
    out
}

/// `yard source trust|untrust`: brew's own trust, changed by an explicit command that
/// needs no terminal and no question.
pub async fn change(ctx: &Ctx, target: String, tap: bool, grant: bool) -> Outcome {
    let (method, done) = if grant {
        (methods::SOURCES_TRUST, format!("Trusted {target}.\n"))
    } else {
        (methods::SOURCES_UNTRUST, format!("Untrusted {target}.\n"))
    };
    run_job(ctx, method, SourceParams { target, tap }, || done).await
}

/// The formula and tap a `FORMULA_UNTRUSTED` error names, when it names both.
fn untrusted(e: &YardError) -> Option<(String, String)> {
    if e.code != ErrorCode::FormulaUntrusted {
        return None;
    }
    let data = e.data.as_ref()?;
    let field = |key: &str| data.get(key)?.as_str().map(str::to_string);
    Some((field("formula")?, field("tap")?))
}

/// `Trust php@8.3 (from the shivammathur/php tap)? [y/N] `.
fn question(formula: &str, tap: &str) -> String {
    format!("Trust {formula} (from the {tap} tap)? [y/N] ")
}

/// Runs `call`; a person who can answer is asked about an untrusted formula (`trust_core`).
/// With no terminal, or `--json`, the error is the result. `--yes` never answers this
/// question: trust is granted only by `yard source trust` or by a person saying yes here.
pub async fn with_trust<T, F, Fut>(ctx: &Ctx, call: F) -> Result<T, CliError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, CliError>>,
{
    if !interactive(ctx) {
        return call().await;
    }
    let run_trust = |formula: String| async move {
        let params = SourceParams { target: formula.clone(), tap: false };
        let done = format!("Trusted {formula}.\n");
        finished(run_job(ctx, methods::SOURCES_TRUST, params, || done).await)
    };
    trust_core(call, confirm, run_trust).await
}

/// The decision logic of `with_trust`: when `call` fails with `FORMULA_UNTRUSTED`, `ask`
/// the question; yes runs `run_trust` for the formula and calls once more, and that second
/// answer is the result whatever it is (no second question). No returns the original
/// error. The question names the formula and tap, so nothing else is printed first and the
/// error is shown once, by the caller.
async fn trust_core<T, F, Fut, A, R, RFut>(call: F, ask: A, run_trust: R) -> Result<T, CliError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, CliError>>,
    A: Fn(&str) -> bool,
    R: Fn(String) -> RFut,
    RFut: Future<Output = Result<(), CliError>>,
{
    match call().await {
        Err(CliError::Yard(e)) => {
            let Some((formula, tap)) = untrusted(&e) else { return Err(e.into()) };
            if !ask(&question(&formula, &tap)) {
                return Err(e.into());
            }
            run_trust(formula).await?;
            call().await
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_prompt_names_the_formula_and_its_tap() {
        let e = YardError::new(ErrorCode::FormulaUntrusted, "x")
            .with_data(json!({"formula": "shivammathur/php/php@8.3", "tap": "shivammathur/php"}));
        let (formula, tap) = untrusted(&e).unwrap();
        assert_eq!(
            question(&formula, &tap),
            "Trust shivammathur/php/php@8.3 (from the shivammathur/php tap)? [y/N] "
        );
    }

    #[test]
    fn only_a_complete_untrusted_error_is_asked_about() {
        let bare = YardError::new(ErrorCode::FormulaUntrusted, "x");
        assert!(untrusted(&bare).is_none(), "no data, nothing to name");
        let half = bare.clone().with_data(json!({"formula": "a/b/c"}));
        assert!(untrusted(&half).is_none());
        let other =
            YardError::new(ErrorCode::JobFailed, "x").with_data(json!({"formula": "a/b/c", "tap": "a/b"}));
        assert!(untrusted(&other).is_none());
    }

    fn untrusted_error() -> CliError {
        let e = YardError::new(ErrorCode::FormulaUntrusted, "php@8.3 is not trusted")
            .with_data(json!({"formula": "shivammathur/php/php@8.3", "tap": "shivammathur/php"}));
        e.into()
    }

    /// Counters for a `trust_core` run: calls, questions, trust runs, and the answers.
    struct Run {
        calls: std::cell::Cell<u32>,
        asked: std::cell::RefCell<Vec<String>>,
        trusted: std::cell::RefCell<Vec<String>>,
    }

    impl Run {
        fn new() -> Self {
            Run { calls: 0.into(), asked: vec![].into(), trusted: vec![].into() }
        }

        /// `calls_failing` calls fail untrusted, the rest succeed.
        async fn go(&self, answer: bool, calls_failing: u32) -> Result<u32, CliError> {
            trust_core(
                || async {
                    self.calls.set(self.calls.get() + 1);
                    if self.calls.get() <= calls_failing {
                        Err(untrusted_error())
                    } else {
                        Ok(self.calls.get())
                    }
                },
                |q| {
                    self.asked.borrow_mut().push(q.to_string());
                    answer
                },
                |formula| async move {
                    self.trusted.borrow_mut().push(formula);
                    Ok(())
                },
            )
            .await
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yes_trusts_the_formula_and_retries_once() {
        let run = Run::new();
        assert_eq!(run.go(true, 1).await.unwrap(), 2);
        assert_eq!(run.calls.get(), 2);
        assert_eq!(*run.trusted.borrow(), ["shivammathur/php/php@8.3"]);
        assert_eq!(run.asked.borrow().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_returns_the_original_error_without_trusting() {
        let run = Run::new();
        let Err(CliError::Yard(e)) = run.go(false, 1).await else { panic!("expected the error") };
        assert_eq!(e.code, ErrorCode::FormulaUntrusted);
        assert_eq!(e.message, "php@8.3 is not trusted");
        assert_eq!((run.calls.get(), run.trusted.borrow().len()), (1, 0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn still_untrusted_after_yes_is_not_asked_again() {
        let run = Run::new();
        let Err(CliError::Yard(e)) = run.go(true, 5).await else { panic!("expected the error") };
        assert_eq!(e.code, ErrorCode::FormulaUntrusted);
        assert_eq!(run.calls.get(), 2, "one retry, no more");
        assert_eq!(run.asked.borrow().len(), 1, "no second question");
        assert_eq!(run.trusted.borrow().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn other_errors_and_successes_are_not_asked_about() {
        let run = Run::new();
        assert_eq!(run.go(true, 0).await.unwrap(), 1);
        assert!(run.asked.borrow().is_empty());
        let other = trust_core(
            || async { Err::<(), _>(CliError::Yard(YardError::new(ErrorCode::JobFailed, "x"))) },
            |_| panic!("asked"),
            |_| async { Ok(()) },
        )
        .await;
        assert!(matches!(other, Err(CliError::Yard(e)) if e.code == ErrorCode::JobFailed));
    }
}
