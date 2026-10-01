//! Nonblocking capture admission, bounded by unacknowledged audio rather than callbacks.
use crate::engine::StreamInput;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, OnceLock,
};
use std::time::Instant;

/// Ten minutes of mono 16 kHz f32 audio (38.4 MB), including the in-flight feed.
pub const STREAM_BACKLOG_MAX_SAMPLES: usize = 10 * 60 * 16_000;
pub const STREAM_LAG_WARNING_SECONDS: f64 = 3.;

#[derive(Default)]
struct Budget {
    samples: AtomicUsize,
    finish: OnceLock<(Instant, usize)>,
}
#[derive(Clone)]
pub struct Sender {
    send: mpsc::Sender<StreamInput>,
    budget: Arc<Budget>,
    limit: usize,
}
pub struct Receiver {
    receive: mpsc::Receiver<StreamInput>,
    budget: Arc<Budget>,
}
pub fn channel() -> (Sender, Receiver) {
    channel_with_limit(STREAM_BACKLOG_MAX_SAMPLES)
}
fn channel_with_limit(limit: usize) -> (Sender, Receiver) {
    let (send, receive) = mpsc::channel();
    let budget = Arc::new(Budget::default());
    (
        Sender {
            send,
            budget: budget.clone(),
            limit,
        },
        Receiver { receive, budget },
    )
}
impl Sender {
    pub fn try_send(&self, input: StreamInput) -> Result<(), mpsc::TrySendError<StreamInput>> {
        let samples = match &input {
            StreamInput::Audio { samples, .. } => samples.len(),
            _ => 0,
        };
        if samples == 0 && matches!(input, StreamInput::Audio { .. }) {
            return Ok(());
        }
        if self
            .budget
            .samples
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(samples).filter(|n| *n <= self.limit)
            })
            .is_err()
        {
            return Err(mpsc::TrySendError::Full(input));
        }
        if matches!(input, StreamInput::Finish) {
            let _ = self
                .budget
                .finish
                .set((Instant::now(), self.budget.samples.load(Ordering::Acquire)));
        }
        self.send.send(input).map_err(|error| {
            self.budget.samples.fetch_sub(samples, Ordering::AcqRel);
            mpsc::TrySendError::Disconnected(error.0)
        })
    }
    #[cfg(test)]
    pub fn send(&self, input: StreamInput) -> Result<(), mpsc::TrySendError<StreamInput>> {
        self.try_send(input)
    }
}
impl Receiver {
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<StreamInput, mpsc::RecvTimeoutError> {
        self.receive.recv_timeout(timeout)
    }
    pub fn acknowledge(&self, samples: usize) {
        self.budget.samples.fetch_sub(samples, Ordering::AcqRel);
    }
    pub fn backlog_seconds(&self) -> f64 {
        self.budget.samples.load(Ordering::Acquire) as f64 / 16_000.
    }
    pub fn finish(&self) -> Option<(Instant, f64)> {
        self.budget
            .finish
            .get()
            .map(|(t, n)| (*t, *n as f64 / 16_000.))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_budget_includes_inflight_audio_and_reserves_controls() {
        let (send, receive) = channel_with_limit(2000);
        send.try_send(StreamInput::audio(vec![0.; 2000])).unwrap();
        let _inflight = receive.recv_timeout(std::time::Duration::ZERO).unwrap();
        assert!(matches!(
            send.try_send(StreamInput::audio(vec![0.; 1])),
            Err(mpsc::TrySendError::Full(_))
        ));
        send.try_send(StreamInput::Finish).unwrap();
        assert_eq!(receive.finish().unwrap().1, 0.125);
        receive.acknowledge(2000);
        assert_eq!(receive.backlog_seconds(), 0.);
        assert!(matches!(
            receive.recv_timeout(std::time::Duration::ZERO).unwrap(),
            StreamInput::Finish
        ));
    }
}
