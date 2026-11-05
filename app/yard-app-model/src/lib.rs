//! What Yard.app decides, kept apart from how it draws: refresh bookkeeping, job records, hint
//! actions, health, install plans and onboarding facts. No GPUI, no tokio, no file or process
//! access, and no clock: `now` is always a parameter, so every rule is a plain `cargo test`.
