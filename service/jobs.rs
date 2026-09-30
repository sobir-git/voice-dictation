//! The coordinator's source of truth. Pending and recording state are derived,
//! never balanced with increments/decrements in unrelated completion handlers.
use crate::{audio::Capture, config::Config, engine::StreamInput};
use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicBool, mpsc, Arc, OnceLock},
    time::Instant,
};
pub type JobId = u64;
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Starting,
    Recording,
    Finalizing,
    Saving,
    Resolving,
    Transcribing,
    Persisting,
    Ready,
    Delivering,
}
impl Stage {
    pub fn recording(self) -> bool {
        matches!(self, Self::Starting | Self::Recording)
    }
    fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Starting, Self::Recording | Self::Finalizing)
                | (Self::Recording, Self::Finalizing)
                | (Self::Finalizing, Self::Saving)
                | (Self::Saving, Self::Transcribing)
                | (Self::Resolving, Self::Transcribing)
                | (Self::Transcribing, Self::Persisting)
                | (Self::Persisting, Self::Ready)
                | (Self::Ready, Self::Delivering)
        )
    }
}
pub struct Transcript {
    pub text: Result<String, String>,
    pub seconds: f64,
    pub model: String,
}
pub struct Job {
    pub id: JobId,
    pub generation: u64,
    pub request_id: Option<u64>,
    pub cancelled: Arc<AtomicBool>,
    pub stage: Stage,
    pub config: Config,
    pub owner: Option<u64>,
    pub history_id: Option<i64>,
    pub capture: Option<Capture>,
    pub capture_ready: bool,
    pub stream: Option<mpsc::SyncSender<StreamInput>>,
    pub finished: Arc<OnceLock<Instant>>,
    pub started: Instant,
    stage_started: Instant,
    pub result: Option<Transcript>,
}
impl Job {
    pub fn recover_stream(&mut self, reason: &str) {
        log::warn!("Stream recovery: job={} history={:?} reason={reason} action=cancel_stream_then_transcribe_saved_audio", self.id, self.history_id);
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.stream = None;
        self.result = None;
    }

    pub fn transition(&mut self, stage: Stage) {
        assert!(
            self.stage.allows(stage),
            "invalid dictation transition {:?} -> {stage:?}",
            self.stage
        );
        let _context = crate::logging::context(
            serde_json::json!({"job_id":self.id,"client_id":self.owner,"request_id":self.request_id,
                "event":"dictation_stage", "from":self.stage,"to":stage,
                "stage_ms":self.stage_started.elapsed().as_secs_f64()*1000.,
                "job_elapsed_ms":self.started.elapsed().as_secs_f64()*1000.,
                "since_recording_stop_ms":self.finished.get().map(|t|t.elapsed().as_secs_f64()*1000.)}),
        );
        log::info!(
            "Dictation transition: job={} from={:?} to={stage:?} history={:?}",
            self.id,
            self.stage,
            self.history_id
        );
        self.stage = stage;
        self.stage_started = Instant::now();
    }
}
#[derive(Default)]
pub struct Jobs {
    next: JobId,
    pub generation: u64,
    pub listening: bool,
    pub active: BTreeMap<JobId, Job>,
}
impl Jobs {
    pub fn listening() -> Self {
        Self {
            listening: true,
            ..Self::default()
        }
    }
    pub fn insert(
        &mut self,
        config: Config,
        stage: Stage,
        owner: Option<u64>,
        history_id: Option<i64>,
    ) -> JobId {
        self.next += 1;
        let id = self.next;
        self.active.insert(
            id,
            Job {
                id,
                generation: self.generation,
                request_id: crate::logging::current_context()["request_id"].as_u64(),
                cancelled: Arc::new(AtomicBool::new(false)),
                config,
                stage,
                owner,
                history_id,
                capture: None,
                capture_ready: false,
                stream: None,
                finished: Arc::new(OnceLock::new()),
                started: Instant::now(),
                stage_started: Instant::now(),
                result: None,
            },
        );
        log::info!(
            "Dictation admitted: job={id} stage={stage:?} generation={} history={history_id:?}",
            self.generation
        );
        id
    }
    pub fn recording(&self) -> Option<JobId> {
        self.active
            .values()
            .find(|j| j.stage.recording())
            .map(|j| j.id)
    }
    pub fn pending(&self) -> usize {
        self.active
            .values()
            .filter(|j| !j.stage.recording())
            .count()
    }
    pub fn outputting(&self) -> bool {
        self.active.values().any(|j| j.stage == Stage::Delivering)
    }
    pub fn busy(&self) -> bool {
        !self.active.is_empty()
    }
    pub fn can_record(&self) -> bool {
        self.listening && self.recording().is_none() && !self.outputting() && self.active.len() < 4
    }
    pub fn complete(&mut self, id: JobId, outcome: &str) -> Option<Job> {
        let job = self.active.remove(&id)?;
        let _context = crate::logging::context(
            serde_json::json!({"job_id":id,"client_id":job.owner,"request_id":job.request_id,
                "event":"dictation_terminal", "outcome":outcome,"stage":job.stage,
                "last_stage_ms":job.stage_started.elapsed().as_secs_f64()*1000.,
                "since_recording_stop_ms":job.finished.get().map(|t|t.elapsed().as_secs_f64()*1000.)}),
        );
        log::info!(
            "Dictation terminal: job={id} outcome={outcome} stage={:?} elapsed={:.3}s history={:?}",
            job.stage,
            job.started.elapsed().as_secs_f64(),
            job.history_id
        );
        Some(job)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_completion_cannot_release_another_job() {
        let root = tempfile::tempdir().unwrap();
        let config = Config::at(root.path()).unwrap();
        let mut jobs = Jobs::default();
        let first = jobs.insert(config.clone(), Stage::Transcribing, None, None);
        let second = jobs.insert(config, Stage::Transcribing, None, None);
        assert!(jobs.complete(first, "failed").is_some());
        assert!(jobs.complete(first, "failed").is_none());
        assert_eq!(jobs.pending(), 1);
        assert!(jobs.active.contains_key(&second));
    }
    #[test]
    fn stages_derive_recording_and_pending() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = Jobs {
            listening: true,
            ..Jobs::default()
        };
        let id = jobs.insert(
            Config::at(root.path()).unwrap(),
            Stage::Starting,
            None,
            None,
        );
        assert!(!jobs.can_record());
        assert_eq!(jobs.pending(), 0);
        jobs.active
            .get_mut(&id)
            .unwrap()
            .transition(Stage::Finalizing);
        assert!(jobs.can_record());
        assert_eq!(jobs.pending(), 1);
    }
}
