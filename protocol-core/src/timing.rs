//! Opt-in bounded local stage timings. No packets, credentials or source text.
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::Instant,
};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Capture,
    Projection,
    Encoding,
    ResourceClosure,
    ReplicaApplication,
    Render,
    Preparation,
    Composition,
    NativePresentation,
    OutputConversion,
    OutboundQueue,
}
#[derive(Clone, Debug, Serialize)]
pub struct Sample {
    pub stage: Stage,
    pub revision: u64,
    pub elapsed_us: u64,
    pub count: usize,
}
#[derive(Serialize)]
pub struct Timings {
    capacity: usize,
    dropped: u64,
    samples: VecDeque<Sample>,
}
impl Timings {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, 16384),
            dropped: 0,
            samples: VecDeque::new(),
        }
    }
    pub fn record(&mut self, sample: Sample) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.samples.push_back(sample);
    }
}
static TIMINGS: OnceLock<Option<Mutex<Timings>>> = OnceLock::new();
fn recorder() -> Option<&'static Mutex<Timings>> {
    // Instant is not supported on the bare WASM target. Browser boundaries use
    // performance.now() and the separately bounded JavaScript recorder.
    TIMINGS
        .get_or_init(|| {
            if cfg!(target_arch = "wasm32") {
                return None;
            }
            std::env::var("RUI_TIMINGS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(|capacity| Mutex::new(Timings::new(capacity)))
        })
        .as_ref()
}
pub struct Span {
    started: Option<Instant>,
    stage: Stage,
    revision: u64,
    count: usize,
}
pub fn elapsed(stage: Stage, revision: u64, duration: std::time::Duration, count: usize) {
    if let Some(mut timings) = recorder().and_then(|r| r.lock().ok()) {
        timings.record(Sample {
            stage,
            revision,
            elapsed_us: u64::try_from(duration.as_micros()).unwrap_or(u64::MAX),
            count,
        });
    }
}
pub fn span(stage: Stage, revision: u64, count: usize) -> Span {
    Span {
        started: recorder().map(|_| Instant::now()),
        stage,
        revision,
        count,
    }
}
impl Span {
    pub fn discard(&mut self) {
        self.started = None;
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            if let Some(mut timings) = recorder().and_then(|r| r.lock().ok()) {
                timings.record(Sample {
                    stage: self.stage,
                    revision: self.revision,
                    elapsed_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    count: self.count,
                });
            }
        }
    }
}
/// Copy bounded samples into a caller-owned report at a quiescent boundary.
pub fn inspect<T>(read: impl FnOnce(&Timings) -> T) -> Option<T> {
    recorder()?.lock().ok().map(|timings| read(&timings))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_retains_latest_and_counts_drops() {
        let mut timings = Timings::new(2);
        for revision in 0..5 {
            timings.record(Sample {
                stage: Stage::Capture,
                revision,
                elapsed_us: 3,
                count: 1,
            });
        }
        assert_eq!(timings.dropped, 3);
        assert_eq!(
            timings
                .samples
                .iter()
                .map(|s| s.revision)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(Timings::new(usize::MAX).capacity, 16384);
    }
}
