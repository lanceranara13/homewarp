//! How often a sign-in may fail before it has to wait (PLAN.md §6, §11
//! Phase 5). Counted in memory: a Homewarp that is started again forgets, which
//! costs whoever is guessing a restart they cannot cause.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How long failures are remembered for, and so the longest anyone waits.
const WINDOW: Duration = Duration::from_secs(5 * 60);
/// How many keys are kept before those that have run out are cleared away.
const CROWDED: usize = 4096;

/// Failures, by who failed: an address, or an account.
#[derive(Default)]
pub(crate) struct Limiter {
    failures: Mutex<HashMap<String, Failures>>,
}

struct Failures {
    count: u32,
    /// When the first of them was.
    since: Instant,
}

impl Limiter {
    /// How long `who` still has to wait, if it has failed `most` times within
    /// the window. The wait ends when the window that began with its first
    /// failure does.
    pub(crate) fn wait(&self, who: &str, most: u32) -> Option<Duration> {
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let known = failures.get(who)?;
        let left = WINDOW.checked_sub(known.since.elapsed())?;
        (known.count >= most).then_some(left)
    }

    /// Counts one more failure against `who`.
    pub(crate) fn failed(&self, who: &str) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        if failures.len() >= CROWDED {
            failures.retain(|_, known| known.since.elapsed() < WINDOW);
        }
        let now = Instant::now();
        let known = failures.entry(who.to_owned()).or_insert(Failures {
            count: 0,
            since: now,
        });
        // What is older than the window no longer counts.
        if known.since.elapsed() >= WINDOW {
            *known = Failures {
                count: 0,
                since: now,
            };
        }
        known.count += 1;
    }

    /// Forgets what `who` has failed: it has got it right.
    pub(crate) fn passed(&self, who: &str) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(who);
    }
}

#[cfg(test)]
mod tests {
    use super::Limiter;

    #[test]
    fn so_many_failures_and_then_a_wait() {
        let limiter = Limiter::default();
        assert_eq!(limiter.wait("from 203.0.113.50", 3), None);
        for _ in 0..2 {
            limiter.failed("from 203.0.113.50");
        }
        assert_eq!(limiter.wait("from 203.0.113.50", 3), None);
        limiter.failed("from 203.0.113.50");
        let wait = limiter.wait("from 203.0.113.50", 3).unwrap();
        assert!(wait.as_secs() > 290 && wait.as_secs() <= 300);
        // Whoever else is asking is not kept waiting with it.
        assert_eq!(limiter.wait("from 203.0.113.51", 3), None);
        // A more patient count of the same failures has not been reached.
        assert_eq!(limiter.wait("from 203.0.113.50", 20), None);

        limiter.passed("from 203.0.113.50");
        assert_eq!(limiter.wait("from 203.0.113.50", 3), None);
    }
}
