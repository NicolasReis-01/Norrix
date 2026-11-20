use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use norrix_scene::{EntityId, Patch};

use crate::{
    ErrorKind, PipelineConfig, PipelineError, Progress, ProgressSink, Stage, StopToken,
    accelerator::AcceleratorGate,
    progress,
    resources::ResourceMonitor,
    stages::{StageInput, Stages},
};

pub(crate) struct StageRunner {
    stages: Stages,
    accelerator: AcceleratorGate,
}

impl StageRunner {
    pub(crate) fn new(
        config: &PipelineConfig,
        translator: norrix_translator::Translator,
        device: &norrix_ml::Device,
        resources: Arc<ResourceMonitor>,
    ) -> Result<Self> {
        Ok(Self {
            stages: Stages::new(config, translator, device)?,
            accelerator: AcceleratorGate::new(device, resources),
        })
    }

    #[tracing::instrument(skip_all)]
    pub(crate) async fn run(&self, job: StageJob) -> StageCompletion {
        let started = Instant::now();
        let page = job.input.page();
        let model = self.stages.model(job.stage).to_owned();
        let outcome = self.run_with_recovery(&job, &model).await;
        match &outcome {
            Ok(StageOutcome::Stopped) => tracing::info!(
                event = "pipeline_stage_finished",
                page = %page,
                stage = %job.stage,
                model,
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                cancellation_reason = job.stop.cancellation_reason().unwrap_or("unknown"),
                result = "cancelled",
            ),
            Ok(StageOutcome::Skipped) => tracing::info!(
                event = "pipeline_stage_finished",
                page = %page,
                stage = %job.stage,
                model,
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                result = "skipped",
            ),
            Ok(StageOutcome::Patch(_)) => tracing::info!(
                event = "pipeline_stage_finished",
                page = %page,
                stage = %job.stage,
                model,
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                result = "output_ready",
            ),
            Err(error) => tracing::error!(
                event = "pipeline_stage_failed",
                page = %page,
                stage = %job.stage,
                model,
                duration_ms = started.elapsed().as_secs_f64() * 1000.0,
                kind = ?error.kind,
                error = %error,
                result = "failed",
            ),
        }
        StageCompletion {
            page,
            stage: job.stage,
            model,
            elapsed: started.elapsed(),
            outcome,
        }
    }

    #[cfg(test)]
    pub(crate) fn should_skip(&self, stage: Stage, input: &StageInput) -> Result<bool> {
        self.stages.skip(stage, input)
    }

    async fn run_with_recovery(
        &self,
        job: &StageJob,
        model: &str,
    ) -> std::result::Result<StageOutcome, PipelineError> {
        if job.stop.stopped() {
            return Ok(StageOutcome::Stopped);
        }
        let skip = self.stages.skip(job.stage, &job.input).map_err(|error| {
            self.stage_error(
                job.stage,
                model,
                AttemptFailure {
                    kind: ErrorKind::Processing,
                    error,
                },
            )
        })?;
        if skip {
            return Ok(StageOutcome::Skipped);
        }
        let permit = tokio::select! {
            biased;
            () = job.stop.cancelled() => return Ok(StageOutcome::Stopped),
            permit = self.accelerator.acquire() => permit,
        };
        if job.stop.stopped() {
            return Ok(StageOutcome::Stopped);
        }
        let first = self.load_and_process(job, model).await;
        let failure = match first {
            Ok(outcome) => return Ok(outcome),
            Err(failure) if is_out_of_memory(&failure.error) && !job.stop.stopped() => failure,
            Err(failure) => return Err(self.stage_error(job.stage, model, failure)),
        };

        drop(permit);
        tracing::warn!(stage = %job.stage, page = %job.input.page(), error = %failure.error, "retrying stage after memory pressure");
        let _metric =
            tracing::info_span!(target: "norrix_metrics", "stage_retry", stage = %job.stage, model);
        let _permit = tokio::select! {
            biased;
            () = job.stop.cancelled() => return Ok(StageOutcome::Stopped),
            permit = self.accelerator.recover(job.stage, &self.stages) => permit,
        };
        if job.stop.stopped() {
            return Ok(StageOutcome::Stopped);
        }
        match self.load_and_process(job, model).await {
            Ok(outcome) => Ok(outcome),
            Err(failure) => Err(self.stage_error(job.stage, model, failure)),
        }
    }

    async fn load_and_process(
        &self,
        job: &StageJob,
        model: &str,
    ) -> std::result::Result<StageOutcome, AttemptFailure> {
        progress::emit(
            job.progress.as_ref(),
            Progress::Loading {
                page: job.input.page(),
                stage: job.stage,
                model: model.to_owned(),
            },
        );
        tokio::select! {
            biased;
            () = job.stop.cancelled() => return Ok(StageOutcome::Stopped),
            result = self.stages.load(job.stage) => {
                result.map_err(|error| AttemptFailure {
                    kind: ErrorKind::ModelLoad,
                    error,
                })?;
            }
        }
        if job.stop.stopped() {
            return Ok(StageOutcome::Stopped);
        }
        progress::emit(
            job.progress.as_ref(),
            Progress::Running {
                page: job.input.page(),
                stage: job.stage,
                model: model.to_owned(),
            },
        );
        tokio::select! {
            biased;
            () = job.stop.cancelled() => Ok(StageOutcome::Stopped),
            result = self.stages.process(job.stage, job.input.clone()) => {
                result
                    .map(|patch| {
                        if patch.is_empty() {
                            StageOutcome::Skipped
                        } else {
                            StageOutcome::Patch(patch)
                        }
                    })
                    .map_err(|error| AttemptFailure {
                        kind: ErrorKind::Processing,
                        error,
                    })
            }
        }
    }

    fn stage_error(&self, stage: Stage, model: &str, failure: AttemptFailure) -> PipelineError {
        self.stages.unload(stage);
        let mut message = match failure.kind {
            ErrorKind::ModelLoad => format!("failed to load {model}"),
            _ => format!("{model} failed"),
        };
        if stage == Stage::Translation {
            message.push_str(
                "; segments without persisted translations remain pending and can be retried",
            );
        }
        PipelineError::new(failure.kind, Some(stage), failure.error.context(message))
    }
}

struct AttemptFailure {
    kind: ErrorKind,
    error: anyhow::Error,
}

fn is_out_of_memory(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        let message = source.to_string().to_ascii_lowercase();
        message.contains("out of memory")
            || message.contains("cuda_error_out_of_memory")
            || message.contains("not enough memory")
    })
}

pub(crate) struct StageJob {
    stage: Stage,
    input: StageInput,
    stop: StopToken,
    progress: Option<ProgressSink>,
}

impl StageJob {
    pub(crate) fn new(
        stage: Stage,
        input: StageInput,
        stop: StopToken,
        progress: Option<ProgressSink>,
    ) -> Self {
        Self {
            stage,
            input,
            stop,
            progress,
        }
    }
}

pub(crate) enum StageOutcome {
    Patch(Patch),
    Skipped,
    Stopped,
}

pub(crate) struct StageCompletion {
    pub(crate) page: EntityId,
    pub(crate) stage: Stage,
    pub(crate) model: String,
    pub(crate) elapsed: Duration,
    pub(crate) outcome: std::result::Result<StageOutcome, PipelineError>,
}
