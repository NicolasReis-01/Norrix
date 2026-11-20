use std::{collections::HashMap, fmt, sync::Arc};

use anyhow::{Context as _, Result};
use norrix_pipeline::{CommitResult, Committer, Progress, RunStatus, StageOutput, StopToken};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, State, ipc::Channel};
use tauri_runtime_cef::CefRuntime;
use tracing::Instrument as _;
use uuid::Uuid;

use super::{ChannelExt as _, Error, canvas::CanvasChannel, project::CurrentProject};
use norrix_desktop::Desktop;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, Type)]
#[serde(transparent)]
pub struct JobId(Uuid);

impl JobId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Job {
    pub id: JobId,
    pub state: JobState,
    #[specta(type = f64)]
    pub completed: usize,
    #[specta(type = f64)]
    pub total: usize,
    #[specta(type = f64)]
    pub skipped: usize,
    #[specta(type = f64)]
    pub failed: usize,
    pub page: Option<norrix_scene::EntityId>,
    pub stage: Option<norrix_pipeline::Stage>,
    pub model: Option<String>,
    pub error: Option<String>,
    pub rotation_failures: Vec<norrix_translator::GeminiKeyFailure>,
    #[specta(type = Option<f64>)]
    pub rotation_successful_key: Option<usize>,
    #[specta(type = Option<f64>)]
    pub rotation_using_key: Option<usize>,
    #[specta(type = Vec<f64>)]
    pub rotation_keys_left: Vec<usize>,
    pub rotation_exhausted: bool,
}

#[derive(Default)]
struct PipelineProgressState {
    completed: usize,
    total: usize,
    skipped: usize,
    failed: usize,
    current: Option<ActiveStage>,
}

#[derive(Clone)]
struct ActiveStage {
    page: norrix_scene::EntityId,
    stage: norrix_pipeline::Stage,
    model: String,
}

impl PipelineProgressState {
    fn start(&mut self, total: usize) {
        *self = Self {
            total,
            ..Self::default()
        };
    }

    fn begin_stage(
        &mut self,
        page: norrix_scene::EntityId,
        stage: norrix_pipeline::Stage,
        model: String,
    ) {
        self.current = Some(ActiveStage { page, stage, model });
    }

    fn current_stage(&self) -> Option<&ActiveStage> {
        self.current.as_ref()
    }

    fn finish(&mut self) {
        self.completed = self.completed.saturating_add(1).min(self.total);
    }

    fn skip(&mut self) {
        self.finish();
        self.skipped = self.skipped.saturating_add(1);
    }

    fn fail(&mut self, completed: usize) {
        self.completed = self.completed.max(completed.min(self.total));
        self.failed = self.failed.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Finished,
    Failed,
    Stopped,
}

#[derive(Default)]
pub(crate) struct Processing {
    pub(crate) stops: Mutex<HashMap<JobId, StopToken>>,
    pub(crate) jobs: Mutex<HashMap<JobId, Job>>,
    pub(crate) inpainting_mask: Mutex<Option<norrix_pipeline::InpaintingMask>>,
}

#[derive(Default)]
pub(crate) struct JobChannel {
    pub(crate) channel: Mutex<Option<Channel<Job>>>,
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process(
    handle: AppHandle<CefRuntime>,
    scope: norrix_pipeline::Scope,
    operation: norrix_pipeline::Operation,
    project: State<'_, CurrentProject>,
    processing: State<'_, Processing>,
    job_channel: State<'_, JobChannel>,
) -> std::result::Result<JobId, Error> {
    let snapshot = project
        .project
        .lock()
        .await
        .as_ref()
        .context("no project is open")?
        .snapshot();
    let id = JobId::new();
    let stop = StopToken::default();
    {
        let mut stops = processing.stops.lock();
        if !stops.is_empty() {
            return Err(anyhow::anyhow!("another process is already running").into());
        }
        stops.insert(id, stop.clone());
    }
    let job = Job {
        id,
        state: JobState::Running,
        completed: 0,
        total: 0,
        skipped: 0,
        failed: 0,
        page: None,
        stage: None,
        model: None,
        error: None,
        rotation_failures: Vec::new(),
        rotation_successful_key: None,
        rotation_using_key: None,
        rotation_keys_left: Vec::new(),
        rotation_exhausted: false,
    };
    processing.jobs.lock().insert(id, job.clone());
    job_channel.channel.publish(job);

    let pipeline = handle.state::<norrix_pipeline::Pipeline>().inner().clone();
    let task_handle = handle.clone();
    let inpainting_mask = processing.inpainting_mask.lock().take();
    tracing::info!(
        event = "pipeline_job_started",
        job_id = %id,
        scope = ?scope,
        operation = ?operation,
        result = "started",
    );
    let job_span = tracing::info_span!("pipeline_job", job_id = %id);
    let task = async move {
        let progress = Arc::new(Mutex::new(PipelineProgressState::default()));
        let progress_handle = task_handle.clone();
        let mut request = norrix_pipeline::Request {
            operation,
            scope,
            stop: stop.clone(),
            progress: None,
            inpainting_mask,
        };
        let scan_project_translations =
            should_scan_project_translations(&request.scope, &request.operation);
        request.progress = Some(Arc::new(move |event| {
            let mut progress = progress.lock();
            let update = match event {
                Progress::Started { pages, stages } => {
                    tracing::info!(
                        target: "norrix_metrics",
                        metric = "pipeline_start",
                        page_count = pages.len(),
                        stage_count = stages.len(),
                    );
                    progress.start(pages.len().saturating_mul(stages.len()));
                    true
                }
                Progress::Loading { page, stage, model } => {
                    tracing::info!(
                        target: "norrix_metrics",
                        metric = "stage_loading",
                        stage = %stage,
                        model,
                    );
                    progress.begin_stage(page, stage, model);
                    true
                }
                Progress::Finished {
                    page,
                    stage,
                    model,
                    elapsed,
                } => {
                    if stage != norrix_pipeline::Stage::Translation {
                        tracing::info!(
                            target: "norrix_metrics",
                            metric = "model_run",
                            stage = %stage,
                            model,
                            duration_ms = elapsed.as_secs_f64() * 1000.0,
                        );
                    }
                    progress.finish();
                    if progress.current.is_none() {
                        progress.begin_stage(page, stage, model);
                    }
                    true
                }
                Progress::Skipped { page, stage } => {
                    tracing::info!(
                        target: "norrix_metrics",
                        metric = "stage_skip",
                        stage = %stage,
                    );
                    progress.skip();
                    progress.begin_stage(page, stage, String::new());
                    true
                }
                Progress::Failed {
                    page,
                    stage,
                    model,
                    completed,
                } => {
                    progress.fail(completed);
                    if progress.current.is_none() {
                        progress.begin_stage(page, stage, model);
                    }

                    true
                }
                Progress::Running { page, stage, model } => {
                    tracing::info!(
                        target: "norrix_metrics",
                        metric = "stage_running",
                        stage = %stage,
                        model,
                    );
                    progress.begin_stage(page, stage, model);
                    true
                }
            };
            if update {
                let active = progress.current_stage().cloned();
                let job = {
                    let processing = progress_handle.state::<Processing>();
                    let mut jobs = processing.jobs.lock();
                    jobs.get_mut(&id).map(|job| {
                        job.completed = progress.completed;
                        job.total = progress.total;
                        job.skipped = progress.skipped;
                        job.failed = progress.failed;
                        job.page = active.as_ref().map(|stage| stage.page);
                        job.stage = active.as_ref().map(|stage| stage.stage);
                        job.model = active.map(|stage| stage.model);
                        job.clone()
                    })
                };
                if let Some(job) = job {
                    progress_handle.state::<JobChannel>().channel.publish(job);
                }
            }
        }));

        struct PipelineCommitter {
            handle: AppHandle<CefRuntime>,
            job_id: String,
        }

        #[async_trait::async_trait]
        impl Committer for PipelineCommitter {
            async fn commit(&mut self, output: StageOutput) -> Result<CommitResult> {
                let (commit, page) = {
                    let projects = self.handle.state::<CurrentProject>();
                    let mut projects = projects.project.lock().await;
                    let project = projects.as_mut().context("no project is open")?;
                    let Some(commit) = project.commit_rebased(output.patch).await? else {
                        return Ok(CommitResult::Conflict);
                    };
                    project.record_commit(&commit);
                    let page = project.active_page();
                    (commit, page)
                };
                let snapshot = commit.snapshot.clone();
                let desktop = self.handle.state::<Desktop>();
                let sync_accepted = desktop
                    .synchronize_with_context(
                        &commit.snapshot,
                        page,
                        &commit,
                        Some(&self.job_id),
                        Some(output.page),
                    )
                    .await?;
                let canvas = desktop.canvas_state();
                tracing::info!(
                    event = "pipeline_canvas_sync_returned",
                    trace_id = %canvas.trace_id,
                    job_id = %self.job_id,
                    processed_page = %output.page,
                    active_page = %page.map_or_else(|| "none".to_owned(), |id| id.to_string()),
                    published_page = %canvas.page.map_or_else(|| "none".to_owned(), |id| id.to_string()),
                    published_revision = %canvas.revision.map_or_else(|| "none".to_owned(), |revision| revision.to_string()),
                    published_generation = canvas.generation,
                    sync_accepted,
                    result = if sync_accepted { "accepted" } else { "not_applied" },
                );
                super::publish_canvas(
                    &self.handle.state::<CanvasChannel>().channel,
                    canvas,
                    Some(&self.job_id),
                    Some(output.page),
                );
                Ok(CommitResult::Applied(snapshot))
            }
        }

        let job_id = id.to_string();
        let mut committer = PipelineCommitter {
            handle: task_handle.clone(),
            job_id,
        };
        let progress_sink = request.progress.clone();
        let mut rotation_events = norrix_translator::subscribe_gemini_rotation_events();
        let mut result = {
            let execution = pipeline.execute(snapshot, request, &mut committer);
            tokio::pin!(execution);
            loop {
                tokio::select! {
                    result = &mut execution => break result,
                    event = rotation_events.recv() => {
                        if let Ok(event) = event {
                            apply_gemini_rotation_event(&task_handle, id, event);
                        }
                    }
                }
            }
        };
        while let Ok(event) = rotation_events.try_recv() {
            apply_gemini_rotation_event(&task_handle, id, event);
        }
        let completed_initial_run =
            matches!(&result, Ok(report) if report.status == RunStatus::Completed);
        let rotation_recovered = {
            let processing = task_handle.state::<Processing>();
            processing.jobs.lock().get(&id).is_some_and(|job| {
                !job.rotation_failures.is_empty() && job.rotation_successful_key.is_some()
            })
        };
        if scan_project_translations
            && completed_initial_run
            && rotation_recovered
            && !stop.stopped()
        {
            let latest_snapshot = {
                let projects = task_handle.state::<CurrentProject>();
                let projects = projects.project.lock().await;
                projects.as_ref().map(|project| project.snapshot())
            };
            if let Some(snapshot) = latest_snapshot {
                let request = norrix_pipeline::Request {
                    operation: norrix_pipeline::Operation::Only {
                        stage: norrix_pipeline::Stage::Translation,
                    },
                    scope: norrix_pipeline::Scope::Project,
                    stop: stop.clone(),
                    progress: progress_sink,
                    inpainting_mask: None,
                };
                result = {
                    let execution = pipeline.execute(snapshot, request, &mut committer);
                    tokio::pin!(execution);
                    loop {
                        tokio::select! {
                            result = &mut execution => break result,
                            event = rotation_events.recv() => {
                                if let Ok(event) = event {
                                    apply_gemini_rotation_event(&task_handle, id, event);
                                }
                            }
                        }
                    }
                };
                while let Ok(event) = rotation_events.try_recv() {
                    apply_gemini_rotation_event(&task_handle, id, event);
                }
            } else {
                tracing::warn!(
                    event = "translation_resume_project_unavailable",
                    job_id = %id,
                    result = "skipped",
                );
            }
        }
        let (stopped, error) = match result {
            Ok(report) => (report.status == RunStatus::Stopped, None),
            Err(error) => {
                tracing::error!(
                    event = "pipeline_job_failed",
                    stage = ?error.stage,
                    kind = ?error.kind,
                    error = %error,
                    result = "failed",
                );
                (false, Some(format!("{error:#}")))
            }
        };
        tracing::info!(
            target: "norrix_metrics",
            event = "pipeline_job_finished",
            job_id = %id,
            metric = "pipeline_result",
            outcome = if stopped {
                "stopped"
            } else if error.is_some() {
                "failed"
            } else {
                "completed"
            },
        );
        tracing::info!(
            event = "pipeline_job_completed",
            job_id = %id,
            cancellation_reason = stop.cancellation_reason().unwrap_or("none"),
            outcome = if stopped {
                "stopped"
            } else if error.is_some() {
                "failed"
            } else {
                "completed"
            },
        );
        task_handle.state::<Processing>().stops.lock().remove(&id);
        let job = task_handle
            .state::<Processing>()
            .jobs
            .lock()
            .remove(&id)
            .map(|mut job| {
                job.state = if stopped {
                    JobState::Stopped
                } else if error.is_some() {
                    JobState::Failed
                } else {
                    JobState::Finished
                };
                job.error = error;
                job
            });
        if let Some(job) = job {
            task_handle.state::<JobChannel>().channel.publish(job);
        }
    };
    drop(tokio::spawn(task.instrument(job_span)));
    Ok(id)
}

fn apply_gemini_rotation_event(
    handle: &AppHandle<CefRuntime>,
    id: JobId,
    event: norrix_translator::GeminiRotationEvent,
) {
    let job = {
        let processing = handle.state::<Processing>();
        let mut jobs = processing.jobs.lock();
        jobs.get_mut(&id).map(|job| {
            apply_gemini_rotation_event_to_job(job, event);
            job.clone()
        })
    };
    if let Some(job) = job {
        handle.state::<JobChannel>().channel.publish(job);
    }
}

fn should_scan_project_translations(
    scope: &norrix_pipeline::Scope,
    operation: &norrix_pipeline::Operation,
) -> bool {
    use norrix_pipeline::{Operation, Scope, Stage};

    !matches!(scope, Scope::Project)
        && match operation {
            Operation::Full => true,
            Operation::Through { stage } => *stage == Stage::Translation,
            Operation::Only { stage } => *stage == Stage::Translation,
            Operation::Stages { stages } => stages.contains(&Stage::Translation),
        }
}

fn apply_gemini_rotation_event_to_job(
    job: &mut Job,
    event: norrix_translator::GeminiRotationEvent,
) {
    match event {
        norrix_translator::GeminiRotationEvent::KeyAttempting {
            key_index,
            keys_left,
        } => {
            job.rotation_using_key = Some(key_index);
            job.rotation_keys_left = keys_left;
        }
        norrix_translator::GeminiRotationEvent::KeyFailed { failure, keys_left } => {
            if !job.rotation_failures.iter().any(|existing| {
                existing.key_index == failure.key_index && existing.reason == failure.reason
            }) {
                job.rotation_failures.push(failure);
            }
            job.rotation_using_key = None;
            job.rotation_keys_left = keys_left;
        }
        norrix_translator::GeminiRotationEvent::Succeeded { key_index } => {
            job.rotation_successful_key = Some(key_index);
            job.rotation_using_key = None;
            job.rotation_keys_left.clear();
        }
        norrix_translator::GeminiRotationEvent::Exhausted => {
            job.rotation_exhausted = true;
            job.rotation_using_key = None;
            job.rotation_keys_left.clear();
        }
    }
    job.rotation_failures
        .sort_by_key(|failure| (failure.key_index, failure.reason.clone()));
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "pipeline_stop",
    skip_all,
    fields(state = "requested")
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn stop_job(
    job: JobId,
    processing: State<'_, Processing>,
) -> std::result::Result<(), Error> {
    let stops = processing.stops.lock();
    let stop = stops
        .get(&job)
        .with_context(|| format!("job {job} is not running"))?;
    tracing::info!(
        target: "norrix_metrics",
        event = "pipeline_job_cancellation_requested",
        job_id = %job,
        cancellation_reason = "user_requested",
        result = "requested",
    );
    tracing::info!(
        event = "pipeline_job_cancellation_requested",
        job_id = %job,
        cancellation_reason = "user_requested",
        result = "requested",
    );
    stop.stop_with_reason("user_requested");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_progress_counts_finished_skipped_and_failed_stages() {
        let mut progress = PipelineProgressState::default();
        progress.start(4);
        progress.finish();
        progress.skip();
        progress.fail(3);
        progress.fail(2);

        assert_eq!(progress.completed, 3);
        assert_eq!(progress.total, 4);
        assert_eq!(progress.skipped, 1);
        assert_eq!(progress.failed, 2);
    }

    #[test]
    fn latest_started_stage_remains_current_after_an_older_stage_finishes() {
        let mut progress = PipelineProgressState::default();
        let page_one = norrix_scene::EntityId::new();
        let page_two = norrix_scene::EntityId::new();
        progress.begin_stage(
            page_one,
            norrix_pipeline::Stage::Ocr,
            "ocr-model".to_owned(),
        );
        progress.begin_stage(
            page_two,
            norrix_pipeline::Stage::Translation,
            "translation-model".to_owned(),
        );
        progress.finish();

        let current = progress.current_stage().unwrap();
        assert_eq!(current.page, page_two);
        assert_eq!(current.stage, norrix_pipeline::Stage::Translation);
        assert_eq!(current.model, "translation-model");
    }

    #[test]
    fn rotation_events_accumulate_per_job_without_exposing_credentials() {
        let mut job = Job {
            id: JobId::new(),
            state: JobState::Running,
            completed: 0,
            total: 5,
            skipped: 0,
            failed: 0,
            page: None,
            stage: Some(norrix_pipeline::Stage::Translation),
            model: Some("gemini-3.8-flash".to_owned()),
            error: None,
            rotation_failures: Vec::new(),
            rotation_successful_key: None,
            rotation_using_key: None,
            rotation_keys_left: Vec::new(),
            rotation_exhausted: false,
        };
        let failures = [
            (3, "Gemini HTTP 429 quota exceeded (requests_per_day)"),
            (1, "Gemini HTTP 503 server error; retryable failure"),
            (2, "Gemini HTTP 401 rejected the API key"),
            (4, "Gemini connection failed before request delivery"),
            (5, "Gemini HTTP 403 project or permission access denied"),
        ];
        for (key_index, reason) in failures {
            apply_gemini_rotation_event_to_job(
                &mut job,
                norrix_translator::GeminiRotationEvent::KeyFailed {
                    failure: norrix_translator::GeminiKeyFailure {
                        key_index,
                        reason: reason.to_owned(),
                    },
                    keys_left: Vec::new(),
                },
            );
        }
        apply_gemini_rotation_event_to_job(
            &mut job,
            norrix_translator::GeminiRotationEvent::Exhausted,
        );

        assert_eq!(
            job.rotation_failures
                .iter()
                .map(|failure| failure.key_index)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        assert!(job.rotation_exhausted);
        assert!(job.rotation_failures.iter().all(|failure| {
            !failure.reason.contains("api-key-secret") && !failure.reason.contains("credential")
        }));

        let mut recovered_job = Job {
            rotation_failures: job.rotation_failures.clone(),
            rotation_using_key: job.rotation_using_key,
            rotation_keys_left: job.rotation_keys_left.clone(),
            ..job
        };
        apply_gemini_rotation_event_to_job(
            &mut recovered_job,
            norrix_translator::GeminiRotationEvent::Succeeded { key_index: 6 },
        );
        assert_eq!(recovered_job.rotation_successful_key, Some(6));
    }

    #[test]
    fn project_translation_scan_is_limited_to_translation_jobs_with_narrow_scopes() {
        use norrix_pipeline::{Operation, Scope, Stage};

        assert!(should_scan_project_translations(
            &Scope::Pages(Vec::new()),
            &Operation::Only {
                stage: Stage::Translation,
            },
        ));
        assert!(should_scan_project_translations(
            &Scope::Region {
                page: norrix_scene::EntityId::new(),
                bounds: norrix_pipeline::Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
            },
            &Operation::Full,
        ));
        assert!(!should_scan_project_translations(
            &Scope::Project,
            &Operation::Only {
                stage: Stage::Translation,
            },
        ));
        assert!(!should_scan_project_translations(
            &Scope::Pages(Vec::new()),
            &Operation::Only { stage: Stage::Ocr },
        ));
        assert!(!should_scan_project_translations(
            &Scope::Entities(Vec::new()),
            &Operation::Through {
                stage: Stage::Inpainting,
            },
        ));
    }

    #[test]
    fn rotation_job_tracks_current_key_and_remaining_key_indices() {
        let mut job = Job {
            id: JobId::new(),
            state: JobState::Running,
            completed: 0,
            total: 1,
            skipped: 0,
            failed: 0,
            page: None,
            stage: Some(norrix_pipeline::Stage::Translation),
            model: None,
            error: None,
            rotation_failures: Vec::new(),
            rotation_successful_key: None,
            rotation_using_key: None,
            rotation_keys_left: Vec::new(),
            rotation_exhausted: false,
        };
        apply_gemini_rotation_event_to_job(
            &mut job,
            norrix_translator::GeminiRotationEvent::KeyAttempting {
                key_index: 1,
                keys_left: vec![2, 4, 5],
            },
        );
        assert_eq!(job.rotation_using_key, Some(1));
        assert_eq!(job.rotation_keys_left, [2, 4, 5]);

        apply_gemini_rotation_event_to_job(
            &mut job,
            norrix_translator::GeminiRotationEvent::KeyFailed {
                failure: norrix_translator::GeminiKeyFailure {
                    key_index: 1,
                    reason: "Gemini HTTP 503 server error; retryable failure".to_owned(),
                },
                keys_left: vec![2, 4, 5],
            },
        );
        assert_eq!(job.rotation_using_key, None);
        assert_eq!(job.rotation_keys_left, [2, 4, 5]);

        apply_gemini_rotation_event_to_job(
            &mut job,
            norrix_translator::GeminiRotationEvent::KeyAttempting {
                key_index: 4,
                keys_left: vec![5],
            },
        );
        assert_eq!(job.rotation_using_key, Some(4));
        assert_eq!(job.rotation_keys_left, [5]);
    }
}
