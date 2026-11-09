//! A small in-memory sliding-window rate limiter, used to cap how often agent
//! tokens can write. State is lost on restart, which is acceptable for this purpose.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::model::{Principal, TokenKind};

pub struct RateLimiter {
    max: u32,
    window: Duration,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    /// `max == 0` disables the limit.
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            max,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Records a hit for `key`, or returns how long to wait if the limit is reached.
    pub fn hit_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        if self.max == 0 {
            return Ok(());
        }
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let q = hits.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
            q.pop_front();
        }
        if q.len() >= self.max as usize {
            let oldest = *q.front().expect("non-empty when full");
            return Err(self.window - now.duration_since(oldest));
        }
        q.push_back(now);
        Ok(())
    }

    /// Applies the limit to agent tokens; human tokens are not limited.
    pub fn check_write(&self, p: &Principal) -> Result<(), Error> {
        if p.token.kind != TokenKind::Agent {
            return Ok(());
        }
        self.hit_at(&p.token.id, Instant::now()).map_err(|wait| {
            Error::TooManyRequests(format!(
                "this agent token reached its limit of {} writes per hour; retry in {} minutes",
                self.max,
                wait.as_secs().div_ceil(60)
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_window() {
        let rl = RateLimiter::new(2, Duration::from_secs(10));
        let t0 = Instant::now();
        assert!(rl.hit_at("a", t0).is_ok());
        assert!(rl.hit_at("a", t0 + Duration::from_secs(1)).is_ok());
        assert_eq!(rl.hit_at("a", t0 + Duration::from_secs(2)), Err(Duration::from_secs(8)));
        assert!(rl.hit_at("b", t0 + Duration::from_secs(2)).is_ok());
        assert!(rl.hit_at("a", t0 + Duration::from_secs(10)).is_ok());
    }

    #[test]
    fn zero_disables() {
        let rl = RateLimiter::new(0, Duration::from_secs(10));
        for _ in 0..100 {
            assert!(rl.hit_at("a", Instant::now()).is_ok());
        }
    }
}
