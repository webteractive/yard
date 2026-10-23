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

/// Runs `call`. When yardd refuses it with `FORMULA_UNTRUSTED` and a person can answer,
/// asks `Trust <formula> (from the <tap> tap)? [y/N]`; yes runs the `sources.trust` job
/// and calls again. Otherwise (no terminal, `--json`, or no) the error is the result.
/// `--yes` never answers this question: trust is granted only by `yard source trust` or
/// by a person saying yes here.
pub async fn with_trust<T, F, Fut>(ctx: &Ctx, call: F) -> Result<T, CliError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, CliError>>,
{
    match call().await {
        Err(CliError::Yard(e)) if interactive(ctx) => {
            let Some((formula, tap)) = untrusted(&e) else { return Err(e.into()) };
            println!("{}.", e.message);
            if !confirm(&question(&formula, &tap)) {
                return Err(e.into());
            }
            let params = SourceParams { target: formula.clone(), tap: false };
            let done = format!("Trusted {formula}.\n");
            finished(run_job(ctx, methods::SOURCES_TRUST, params, || done).await)?;
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
}
