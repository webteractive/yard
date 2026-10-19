//! When to restart a crashed unit (spec §6.1). Pure: the caller passes the clock.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyConfig {
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub failure_window: Duration,
    pub max_failures: u32,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
            max_failures: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    RestartAfter(Duration),
    /// Too many failures inside the window: stop until a manual `yard restart`.
    Fail,
}

#[derive(Debug, Clone)]
pub struct RestartPolicy {
    cfg: PolicyConfig,
    failures: VecDeque<Instant>,
}

impl RestartPolicy {
    pub fn new(cfg: PolicyConfig) -> Self {
        Self { cfg, failures: VecDeque::new() }
    }

    /// Delay is `base * 2^(n-1)` capped at `max`, where n counts failures still inside the window.
    pub fn on_failure(&mut self, now: Instant) -> Decision {
        while self.failures.front().is_some_and(|t| now.duration_since(*t) > self.cfg.failure_window) {
            self.failures.pop_front();
        }
        self.failures.push_back(now);
        let n = self.failures.len() as u32;
        if n >= self.cfg.max_failures {
            return Decision::Fail;
        }
        let factor = 2u32.saturating_pow(n - 1);
        Decision::RestartAfter(self.cfg.backoff_base.saturating_mul(factor).min(self.cfg.backoff_max))
    }

    pub fn reset(&mut self) {
        self.failures.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn defaults_match_spec() {
        let c = PolicyConfig::default();
        assert_eq!(
            (c.backoff_base, c.backoff_max, c.failure_window, c.max_failures),
            (secs(1), secs(30), secs(60), 5)
        );
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let mut p = RestartPolicy::new(PolicyConfig {
            max_failures: 100,
            failure_window: secs(1000),
            ..Default::default()
        });
        let t0 = Instant::now();
        let delays: Vec<Decision> = (0..7).map(|i| p.on_failure(t0 + secs(10 * i))).collect();
        let expected: Vec<Decision> =
            [1, 2, 4, 8, 16, 30, 30].iter().map(|s| Decision::RestartAfter(secs(*s))).collect();
        assert_eq!(delays, expected);
    }

    #[test]
    fn fifth_failure_within_window_fails() {
        let mut p = RestartPolicy::new(PolicyConfig::default());
        let t0 = Instant::now();
        let d: Vec<Decision> = (0..5).map(|i| p.on_failure(t0 + secs(i))).collect();
        assert!(d[..4].iter().all(|d| matches!(d, Decision::RestartAfter(_))), "{d:?}");
        assert_eq!(d[4], Decision::Fail);
    }

    #[test]
    fn failures_outside_window_are_forgotten() {
        let mut p = RestartPolicy::new(PolicyConfig::default());
        let t0 = Instant::now();
        for i in 0..4 {
            p.on_failure(t0 + secs(i));
        }
        assert_eq!(p.on_failure(t0 + secs(3 + 61)), Decision::RestartAfter(secs(1)));
    }

    #[test]
    fn reset_clears_history() {
        let mut p = RestartPolicy::new(PolicyConfig::default());
        let t0 = Instant::now();
        for i in 0..4 {
            p.on_failure(t0 + secs(i));
        }
        p.reset();
        assert_eq!(p.on_failure(t0 + secs(5)), Decision::RestartAfter(secs(1)));
    }
}
