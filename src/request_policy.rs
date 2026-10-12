//! Shared provider pacing. HTTP throttling is an answer, never a retry trigger.
use anyhow::{Result, bail};
use std::{
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

#[derive(Default)]
pub struct RequestPolicy(Mutex<(Option<Instant>, Option<Instant>)>);

pub(crate) fn youtube() -> &'static RequestPolicy {
    static POLICY: std::sync::OnceLock<RequestPolicy> = std::sync::OnceLock::new();
    POLICY.get_or_init(Default::default)
}

impl RequestPolicy {
    pub fn before(&self) -> Result<()> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(until) = state.1.filter(|until| *until > Instant::now()) {
            bail!(
                "The music service is limiting requests. Try again in {} seconds.",
                until.saturating_duration_since(Instant::now()).as_secs() + 1
            );
        }
        if let Some(last) = state.0 {
            std::thread::sleep(Duration::from_millis(125).saturating_sub(last.elapsed()));
        }
        state.0 = Some(Instant::now());
        Ok(())
    }

    pub fn observe(&self, status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap) {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            self.pause(retry_after(headers, SystemTime::now()));
        }
    }

    pub fn pause(&self, duration: Duration) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let until = now
            .checked_add(duration)
            .unwrap_or(now + Duration::from_secs(365 * 86400));
        state.1 = Some(state.1.map_or(until, |old| old.max(until)));
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Duration {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok());
    value
        .and_then(|v| {
            v.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                httpdate::parse_http_date(v)
                    .ok()
                    .map(|date| date.duration_since(now).unwrap_or_default())
            })
        })
        .unwrap_or(Duration::from_secs(60))
        .max(Duration::from_secs(1))
}
