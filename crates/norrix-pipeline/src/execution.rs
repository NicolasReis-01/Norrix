use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context as _, Result, ensure};
use futures::{StreamExt as _, stream::FuturesUnordered};
use norrix_scene::{
    EntityId, EntityOrigin, Origin, PipelineProgress, ProgressKind, ProgressOrigin, ProgressStatus,
    RecognizedFrom, Region, RegionSpec, Snapshot, SourceText, TextLayout, TextRegion, Translation,
};

use crate::{
    CommitResult, Committer, ErrorKind, PipelineError, Progress, ProgressSink, Report, Request,
    RunStatus, Stage, StageOutput, StopToken,
    images::ImageCache,
    progress,
    resources::ResourceMonitor,
    scheduler::Scheduler,
    scope::NormalizedScope,
    stage_runner::{StageCompletion, StageJob, StageOutcome, StageRunner},
    stages::StageInput,
};

pub(crate) struct Execution<'a> {
    runner: Arc<StageRunner>,
    resources: Arc<ResourceMonitor>,
    committer: &'a mut dyn Committer,
    stop: StopToken,
    progress: Option<ProgressSink>,
    scope: NormalizedScope,
    scheduler: Scheduler,
    scene: Snapshot,
    images: BTreeMap<EntityId, Arc<ImageCache>>,
    busy_stages: BTreeSet<Stage>,
    completed: usize,
    failure: Option<PipelineError>,
    base: norrix_scene::Revision,
    started: Instant,
    inpainting_mask: Option<crate::InpaintingMask>,
}

impl<'a> Execution<'a> {
    pub(crate) fn new(
        runner: Arc<StageRunner>,
        resources: Arc<ResourceMonitor>,
        snapshot: Snapshot,
        request: Request,
        committer: &'a mut dyn Committer,
    ) -> std::result::Result<Self, PipelineError> {
        let started = Instant::now();
        let base = snapshot.revision();
        let stages = request
            .operation
            .stages()
            .map_err(|error| PipelineError::new(ErrorKind::InvalidInput, None, error))?;
        let scope = NormalizedScope::new(&snapshot, &request.scope, &stages)
            .map_err(|error| PipelineError::new(ErrorKind::InvalidInput, None, error))?;
        let pages = scope.pages().to_vec();
        if let Some(mask) = request.inpainting_mask.as_ref()
            && (!pages.contains(&mask.page) || !stages.contains(&Stage::Inpainting))
        {
            return Err(PipelineError::new(
                ErrorKind::InvalidInput,
                Some(Stage::Inpainting),
                anyhow::anyhow!("the inpainting mask page is outside the inpainting scope"),
            ));
        }
        progress::emit(
            request.progress.as_ref(),
            Progress::Started {
                pages: pages.clone(),
                stages: stages.clone(),
            },
        );

        Ok(Self {
            runner,
            resources,
            committer,
            stop: request.stop,
            progress: request.progress,
            scope,
            scheduler: Scheduler::new(&pages, &stages),
            scene: snapshot,
            images: BTreeMap::new(),
            busy_stages: BTreeSet::new(),
            completed: 0,
            failure: None,
            base,
            started,
            inpainting_mask: request.inpainting_mask,
        })
    }

    pub(crate) async fn run(mut self) -> std::result::Result<Report, PipelineError> {
        if self.stopped() {
            return Ok(self.report(RunStatus::Stopped));
        }

        self.resources.start();
        self.resources.wait_for_sample().await;

        let runner = self.runner.clone();
        let mut running = FuturesUnordered::new();
        loop {
            while let Some(job) = self.take_ready_job() {
                running.push(runner.run(job));
            }

            let Some(completion) = running.next().await else {
                break;
            };
            self.busy_stages.remove(&completion.stage);
            if self.stopped() {
                continue;
            }
            let page = completion.page;
            let stage = completion.stage;
            let model = completion.model.clone();
            if let Err(error) = self.apply_completion(completion).await {
                if self.stopped() {
                    tracing::info!(
                        event = "pipeline_stage_failure_discarded_after_cancellation",
                        page = %page,
                        stage = %stage,
                        cancellation_reason = self.stop.cancellation_reason().unwrap_or("unknown"),
                        result = "cancelled",
                    );
                    continue;
                }
                let skipped = self.scheduler.fail_page(page, stage);
                self.completed = self.completed.saturating_add(skipped);
                tracing::error!(
                    event = "pipeline_page_failed",
                    page = %page,
                    stage = %stage,
                    model,
                    kind = ?error.kind,
                    error = %error,
                    downstream_work_skipped = skipped.saturating_sub(1),
                    result = "page-failed",
                );
                progress::emit(
                    self.progress.as_ref(),
                    Progress::Failed {
                        page,
                        stage,
                        model,
                        completed: self.completed,
                    },
                );
                if self.failure.is_none() {
                    self.failure = Some(error);
                }
            }
        }

        self.finalize()
    }

    fn take_ready_job(&mut self) -> Option<StageJob> {
        if self.stopped() {
            return None;
        }
        let (page, stage) = self.scheduler.start_next(&self.busy_stages)?;
        self.busy_stages.insert(stage);
        let images = self
            .images
            .entry(page)
            .or_insert_with(|| Arc::new(ImageCache::default()))
            .clone();
        Some(StageJob::new(
            stage,
            StageInput::new(
                self.scene.clone(),
                page,
                self.scope.entities(),
                self.scope.region(page),
                images,
                self.inpainting_mask
                    .as_ref()
                    .filter(|mask| stage == Stage::Inpainting && mask.page == page)
                    .cloned(),
            ),
            self.stop.clone(),
            self.progress.clone(),
        ))
    }

    async fn apply_completion(
        &mut self,
        completion: StageCompletion,
    ) -> std::result::Result<(), PipelineError> {
        let StageCompletion {
            page,
            stage,
            model,
            elapsed,
            outcome,
        } = completion;
        let application_started = Instant::now();
        let outcome = match outcome {
            Err(_error) if self.stopped() => {
                tracing::info!(
                    event = "pipeline_stage_failure_discarded_after_cancellation",
                    page = %page,
                    stage = %stage,
                    cancellation_reason = self.stop.cancellation_reason().unwrap_or("unknown"),
                    result = "cancelled",
                );
                return Ok(());
            }
            Err(error) => {
                if stage == Stage::Translation {
                    tracing::error!(
                        event = "translation_pipeline_stage_finished",
                        page = %page,
                        stage = %stage,
                        model,
                        stage_processing_duration_ms = elapsed.as_secs_f64() * 1000.0,
                        result = "failed",
                    );
                }
                return Err(error);
            }
            Ok(outcome) => outcome,
        };
        let translation_outcome = match &outcome {
            StageOutcome::Stopped => "cancelled",
            StageOutcome::Skipped => "skipped",
            StageOutcome::Patch(_) => "output_ready",
        };
        match outcome {
            StageOutcome::Stopped => {}
            StageOutcome::Skipped => {
                self.commit_stage_progress(page, stage, false, None).await?;
                self.mark_complete(page, stage);
                progress::emit(self.progress.as_ref(), Progress::Skipped { page, stage });
            }
            StageOutcome::Patch(patch) => {
                let previous_scene = self.scene.clone();
                if !self
                    .commit_patch(page, stage, patch, "stage_output")
                    .await?
                {
                    return Ok(());
                }
                if stage == Stage::Translation {
                    let persisted_translation_count =
                        persisted_translation_count(&self.scene, page).map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?;
                    tracing::info!(
                        event = "translation_output_persistence_verified",
                        page = %page,
                        persisted_translation_count,
                        result = if persisted_translation_count == 0 {
                            "missing"
                        } else {
                            "verified"
                        },
                    );
                    if persisted_translation_count == 0 {
                        return Err(PipelineError::new(
                            ErrorKind::InvalidOutput,
                            Some(stage),
                            anyhow::anyhow!(
                                "translation output committed without any non-empty translation"
                            ),
                        ));
                    }
                }
                self.commit_stage_progress(page, stage, true, Some(&previous_scene))
                    .await?;
                self.mark_complete(page, stage);
                progress::emit(
                    self.progress.as_ref(),
                    Progress::Finished {
                        page,
                        stage,
                        model: model.clone(),
                        elapsed,
                    },
                );
            }
        }
        if stage == Stage::Translation {
            let commit_and_marker_duration = application_started.elapsed();
            tracing::info!(
                event = "translation_pipeline_stage_finished",
                page = %page,
                stage = %stage,
                model,
                stage_processing_duration_ms = elapsed.as_secs_f64() * 1000.0,
                commit_and_marker_duration_ms =
                    commit_and_marker_duration.as_secs_f64() * 1000.0,
                duration_ms = (elapsed + commit_and_marker_duration).as_secs_f64() * 1000.0,
                result = translation_outcome,
            );
        }
        Ok(())
    }

    async fn commit_patch(
        &mut self,
        page: EntityId,
        stage: Stage,
        patch: norrix_scene::Patch,
        commit_kind: &'static str,
    ) -> std::result::Result<bool, PipelineError> {
        let patch = patch
            .rebase_on(&self.scene)
            .and_then(|patch| {
                patch.validate_on(&self.scene)?;
                Ok(patch.with_label(format!("Pipeline {stage} for page {page}")))
            })
            .context("failed to rebase stage output onto the latest scene")
            .map_err(|error| PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error))?;
        if self.stopped() {
            return Ok(false);
        }

        let commit_started = Instant::now();
        let next = match self
            .committer
            .commit(StageOutput { page, stage, patch })
            .await
        {
            Ok(CommitResult::Applied(next)) => {
                tracing::info!(
                    event = "pipeline_committer_call_finished",
                    page = %page,
                    stage = %stage,
                    commit_kind,
                    duration_ms = commit_started.elapsed().as_secs_f64() * 1000.0,
                    result = "committed",
                );
                next
            }
            Ok(CommitResult::Conflict) => {
                tracing::warn!(
                    event = "pipeline_committer_call_finished",
                    page = %page,
                    stage = %stage,
                    commit_kind,
                    duration_ms = commit_started.elapsed().as_secs_f64() * 1000.0,
                    result = "conflict",
                );
                return Err(PipelineError::new(
                    ErrorKind::Commit,
                    Some(stage),
                    anyhow::anyhow!(
                        "pipeline {stage} output was superseded by a concurrent document edit"
                    ),
                ));
            }
            Err(error) => {
                tracing::error!(
                    event = "pipeline_committer_call_failed",
                    page = %page,
                    stage = %stage,
                    commit_kind,
                    duration_ms = commit_started.elapsed().as_secs_f64() * 1000.0,
                    result = "failed",
                );
                return Err(PipelineError::new(
                    ErrorKind::Commit,
                    Some(stage),
                    error.context(format!("failed to commit {stage} output for page {page}")),
                ));
            }
        };
        validate_commit(&self.scene, &next)
            .map_err(|error| PipelineError::new(ErrorKind::Commit, Some(stage), error))?;
        self.scene = next;
        Ok(true)
    }

    async fn commit_stage_progress(
        &mut self,
        page: EntityId,
        stage: Stage,
        changed: bool,
        previous_scene: Option<&Snapshot>,
    ) -> std::result::Result<(), PipelineError> {
        let marker_scan_started = Instant::now();
        let kind = match stage {
            Stage::Detection => Some(ProgressKind::Detection),
            Stage::Ocr => Some(ProgressKind::Ocr),
            Stage::Translation => None,
            Stage::Inpainting => Some(ProgressKind::Inpainting),
        };
        let mut page_progress = self
            .scene
            .component::<PipelineProgress>(page)
            .map_err(|error| PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error))?
            .unwrap_or_default();
        let mut changed_page = false;
        if let Some(kind) = kind
            && !page_progress.is_complete(kind)
        {
            page_progress.set(
                kind,
                Some(ProgressStatus::Complete(ProgressOrigin::Automatic)),
            );
            changed_page = true;
        }

        if changed {
            match stage {
                Stage::Detection => {
                    if page_progress.ocr.take().is_some() {
                        changed_page = true;
                    }
                    if page_progress.inpainting.take().is_some() {
                        changed_page = true;
                    }
                }
                Stage::Translation if page_progress.inpainting.take().is_some() => {
                    changed_page = true;
                }
                _ => {}
            }
        }

        let mut translation_progress = Vec::new();
        if changed && stage == Stage::Detection {
            let collect_ids = |snapshot: &Snapshot| -> Result<BTreeSet<EntityId>> {
                let mut ids = BTreeSet::new();
                let Some(group) = snapshot.page(page)?.text_group()? else {
                    return Ok(ids);
                };
                for layer in group.text_layers()? {
                    let content = layer.content()?;
                    let manual = snapshot
                        .component::<EntityOrigin>(layer.id())?
                        .is_some_and(|origin| matches!(origin.origin, Origin::User))
                        || snapshot
                            .component::<TextLayout>(layer.id())?
                            .is_some_and(|layout| matches!(layout.origin, Origin::User))
                        || snapshot.component::<TextLayout>(layer.id())?.is_none();
                    let detected = if let Some(relation) =
                        snapshot.relation_from::<RecognizedFrom>(content.id())?
                    {
                        snapshot
                            .component::<Region>(relation.value().target)?
                            .is_some_and(|region| region.kind == TextRegion::kind())
                    } else {
                        false
                    };
                    if manual || detected {
                        ids.insert(content.id());
                    }
                }
                Ok(ids)
            };
            let previous_ids = previous_scene
                .map(collect_ids)
                .transpose()
                .map_err(|error| PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error))?
                .unwrap_or_default();
            let current_ids = collect_ids(&self.scene).map_err(|error| {
                PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
            })?;
            if current_ids.is_empty() {
                if !page_progress.is_complete(ProgressKind::Translation) {
                    page_progress.set(
                        ProgressKind::Translation,
                        Some(ProgressStatus::Complete(ProgressOrigin::Automatic)),
                    );
                    changed_page = true;
                }
            } else if previous_ids != current_ids {
                if page_progress.status(ProgressKind::Translation).is_some()
                    && page_progress.is_complete(ProgressKind::Translation)
                {
                    page_progress.set(ProgressKind::Translation, None);
                    changed_page = true;
                }
                for content in current_ids.iter() {
                    let mut progress = self
                        .scene
                        .component::<PipelineProgress>(*content)
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?
                        .unwrap_or_default();
                    if !previous_ids.contains(content)
                        || progress.is_complete(ProgressKind::Translation)
                    {
                        progress.set(
                            ProgressKind::Translation,
                            Some(ProgressStatus::Pending(ProgressOrigin::Automatic)),
                        );
                        translation_progress.push((*content, progress));
                    }
                }
            }
        }
        if changed
            && stage == Stage::Ocr
            && let Some(previous_scene) = previous_scene
        {
            let mut source_changed = false;
            if let Some(group) = self
                .scene
                .page(page)
                .and_then(|page| page.text_group())
                .map_err(|error| PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error))?
            {
                for layer in group.text_layers().map_err(|error| {
                    PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                })? {
                    let content = layer.content().map_err(|error| {
                        PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                    })?;
                    let current_source =
                        self.scene
                            .component::<SourceText>(content.id())
                            .map_err(|error| {
                                PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                            })?;
                    let previous_source = previous_scene
                        .component::<SourceText>(content.id())
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?;
                    let did_change = current_source
                        .as_ref()
                        .map(|source| source.text.value.as_str())
                        != previous_source
                            .as_ref()
                            .map(|source| source.text.value.as_str());
                    if !did_change {
                        continue;
                    }
                    source_changed = true;
                    let mut progress = self
                        .scene
                        .component::<PipelineProgress>(content.id())
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?
                        .unwrap_or_default();
                    let has_translation = self
                        .scene
                        .component::<Translation>(content.id())
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?
                        .is_some_and(|translation| !translation.text.value.trim().is_empty());
                    if progress
                        .status(ProgressKind::Translation)
                        .is_some_and(|status| matches!(status, ProgressStatus::Complete(_)))
                        || has_translation
                    {
                        progress.set(
                            ProgressKind::Translation,
                            Some(ProgressStatus::Pending(ProgressOrigin::Automatic)),
                        );
                        translation_progress.push((content.id(), progress));
                    }
                }
            }
            if source_changed && page_progress.inpainting.take().is_some() {
                changed_page = true;
            }
        }
        if changed && stage == Stage::Translation {
            if let Some(group) = self
                .scene
                .page(page)
                .and_then(|page| page.text_group())
                .map_err(|error| PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error))?
            {
                for layer in group.text_layers().map_err(|error| {
                    PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                })? {
                    let content = layer.content().map_err(|error| {
                        PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                    })?;
                    if !self
                        .scope
                        .contains_entity(&self.scene, page, layer.id())
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?
                    {
                        continue;
                    }
                    let has_source = self
                        .scene
                        .component::<SourceText>(content.id())
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?
                        .is_some_and(|source| !source.text.value.trim().is_empty());
                    let has_translation = self
                        .scene
                        .component::<Translation>(content.id())
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?
                        .is_some_and(|translation| !translation.text.value.trim().is_empty());
                    if !has_source || !has_translation {
                        continue;
                    }
                    let mut progress = self
                        .scene
                        .component::<PipelineProgress>(content.id())
                        .map_err(|error| {
                            PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error)
                        })?
                        .unwrap_or_default();
                    if !progress.is_complete(ProgressKind::Translation) {
                        progress.set(
                            ProgressKind::Translation,
                            Some(ProgressStatus::Complete(ProgressOrigin::Automatic)),
                        );
                        translation_progress.push((content.id(), progress));
                    }
                }
            }
        }

        if !changed_page && translation_progress.is_empty() {
            if stage == Stage::Translation {
                tracing::info!(
                    event = "translation_progress_marker_scan_finished",
                    page = %page,
                    stage = %stage,
                    translation_markers = 0,
                    page_marker_changed = false,
                    duration_ms = marker_scan_started.elapsed().as_secs_f64() * 1000.0,
                    result = "no_changes",
                );
            }
            return Ok(());
        }
        if stage == Stage::Translation {
            tracing::info!(
                event = "translation_progress_marker_scan_finished",
                page = %page,
                stage = %stage,
                translation_markers = translation_progress.len(),
                page_marker_changed = changed_page,
                duration_ms = marker_scan_started.elapsed().as_secs_f64() * 1000.0,
                result = "changes_found",
            );
        }
        let marker_build_started = Instant::now();
        let patch = self
            .scene
            .patch(|edit| {
                if changed_page {
                    edit.set(page, &page_progress)?;
                }
                for (content, progress) in &translation_progress {
                    edit.set(*content, progress)?;
                }
                Ok(())
            })
            .map_err(|error| PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error))?;
        if stage == Stage::Translation {
            tracing::info!(
                event = "translation_progress_marker_patch_built",
                page = %page,
                stage = %stage,
                translation_markers = translation_progress.len(),
                duration_ms = marker_build_started.elapsed().as_secs_f64() * 1000.0,
                result = "built",
            );
        }
        self.commit_patch(page, stage, patch, "progress_marker")
            .await?;
        tracing::info!(
            event = "pipeline_progress_persisted",
            page = %page,
            stage = %stage,
            translation_markers = translation_progress.len(),
            page_marker_changed = changed_page,
            result = "committed",
        );
        Ok(())
    }

    fn mark_complete(&mut self, page: EntityId, stage: Stage) {
        if self.scheduler.complete_stage(page, stage) {
            self.images.remove(&page);
        }
        self.completed += 1;
    }

    fn stopped(&self) -> bool {
        self.stop.stopped()
    }

    fn finalize(mut self) -> std::result::Result<Report, PipelineError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if !self.stopped() && self.completed != self.scheduler.total() {
            return Err(PipelineError::new(
                ErrorKind::InvalidOutput,
                None,
                anyhow::anyhow!(
                    "pipeline scheduler stopped after {} of {} work items",
                    self.completed,
                    self.scheduler.total()
                ),
            ));
        }
        let status = if self.stopped() {
            RunStatus::Stopped
        } else {
            RunStatus::Completed
        };
        Ok(self.report(status))
    }

    fn report(&self, status: RunStatus) -> Report {
        Report {
            status,
            base: self.base,
            final_revision: self.scene.revision(),
            completed: self.completed,
            total: self.scheduler.total(),
            elapsed: self.started.elapsed(),
        }
    }
}

fn persisted_translation_count(snapshot: &Snapshot, page_id: EntityId) -> Result<usize> {
    let Some(group) = snapshot.page(page_id)?.text_group()? else {
        return Ok(0);
    };
    let mut count = 0;
    for layer in group.text_layers()? {
        let content = layer.content()?;
        let Some(source) = content.source()? else {
            continue;
        };
        let Some(translation) = content.translation()? else {
            continue;
        };
        if !source.text.value.trim().is_empty() && !translation.text.value.trim().is_empty() {
            count += 1;
        }
    }
    Ok(count)
}

fn validate_commit(previous: &Snapshot, next: &Snapshot) -> Result<()> {
    ensure!(
        previous.project_id() == next.project_id(),
        "committer returned a snapshot from another project"
    );
    ensure!(
        next.revision() > previous.revision(),
        "committer did not advance the scene revision"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct SessionCommitter<'a> {
        session: &'a mut norrix_scene::Session,
        delay: Duration,
        commits: usize,
    }

    impl<'a> SessionCommitter<'a> {
        fn new(session: &'a mut norrix_scene::Session) -> Self {
            Self {
                session,
                delay: Duration::ZERO,
                commits: 0,
            }
        }

        fn with_delay(session: &'a mut norrix_scene::Session, delay: Duration) -> Self {
            Self {
                session,
                delay,
                commits: 0,
            }
        }
    }

    #[async_trait::async_trait]
    impl Committer for SessionCommitter<'_> {
        async fn commit(&mut self, output: StageOutput) -> Result<CommitResult> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.commits += 1;
            Ok(CommitResult::Applied(
                self.session.commit(output.patch).await?.snapshot,
            ))
        }
    }

    struct ConflictCommitter {
        commits: usize,
    }

    #[async_trait::async_trait]
    impl Committer for ConflictCommitter {
        async fn commit(&mut self, _output: StageOutput) -> Result<CommitResult> {
            self.commits += 1;
            Ok(CommitResult::Conflict)
        }
    }

    #[tokio::test]
    async fn failed_stage_keeps_prior_markers_and_is_scheduled_for_retry() {
        let mut session = norrix_scene::Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(
                norrix_scene::PageDraft::new("retry", 100.0, 100.0),
                norrix_scene::At::End,
            )
            .unwrap();
        let content = edit.add_text_content(page, norrix_scene::At::End).unwrap();
        edit.set(
            content,
            &norrix_scene::SourceText {
                text: norrix_scene::Authored::user("source".to_owned()),
                language: None,
            },
        )
        .unwrap();
        edit.add_text_layer(
            page,
            norrix_scene::At::End,
            content,
            &norrix_scene::TextLayout {
                origin: norrix_scene::Origin::User,
                kind: norrix_scene::TextLayoutKind::Paragraph,
                angle_degrees: Some(0.0),
            },
        )
        .unwrap();
        let translated_content = edit.add_text_content(page, norrix_scene::At::End).unwrap();
        edit.set(
            translated_content,
            &SourceText {
                text: norrix_scene::Authored::user("already translated".to_owned()),
                language: None,
            },
        )
        .unwrap();
        edit.set(
            translated_content,
            &Translation {
                text: norrix_scene::Authored::user("translation already saved".to_owned()),
                language: None,
            },
        )
        .unwrap();
        let mut translated_progress = PipelineProgress::default();
        translated_progress.set(
            ProgressKind::Translation,
            Some(ProgressStatus::Complete(ProgressOrigin::Automatic)),
        );
        edit.set(translated_content, &translated_progress).unwrap();
        edit.add_text_layer(
            page,
            norrix_scene::At::End,
            translated_content,
            &norrix_scene::TextLayout {
                origin: norrix_scene::Origin::User,
                kind: norrix_scene::TextLayoutKind::Paragraph,
                angle_degrees: Some(0.0),
            },
        )
        .unwrap();
        session.commit(edit.finish().unwrap()).await.unwrap();

        let device = norrix_ml::Device::cpu();
        let resources = ResourceMonitor::new(&device);
        let translator = norrix_translator::Translator::from_config(
            device.clone(),
            norrix_config::Config::memory(norrix_translator::ProvidersConfig::default()),
        )
        .unwrap();
        let runner = Arc::new(
            StageRunner::new(
                &crate::PipelineConfig::default(),
                translator,
                &device,
                resources.clone(),
            )
            .unwrap(),
        );
        let mut committer = SessionCommitter::new(&mut session);
        let mut execution = Execution::new(
            runner.clone(),
            resources.clone(),
            committer.session.snapshot(),
            Request {
                operation: crate::Operation::Stages {
                    stages: vec![Stage::Detection, Stage::Ocr, Stage::Translation],
                },
                ..Request::default()
            },
            &mut committer,
        )
        .unwrap();

        for stage in [Stage::Detection, Stage::Ocr] {
            execution
                .apply_completion(StageCompletion {
                    page,
                    stage,
                    model: "mock".to_owned(),
                    elapsed: Duration::ZERO,
                    outcome: Ok(StageOutcome::Skipped),
                })
                .await
                .unwrap();
        }
        let failure = PipelineError::new(
            ErrorKind::Processing,
            Some(Stage::Translation),
            anyhow::anyhow!("simulated inference failure"),
        );
        assert!(
            execution
                .apply_completion(StageCompletion {
                    page,
                    stage: Stage::Translation,
                    model: "mock".to_owned(),
                    elapsed: Duration::ZERO,
                    outcome: Err(failure),
                })
                .await
                .is_err()
        );

        let page_progress = execution
            .scene
            .component::<PipelineProgress>(page)
            .unwrap()
            .unwrap();
        assert!(page_progress.is_complete(ProgressKind::Detection));
        assert!(page_progress.is_complete(ProgressKind::Ocr));
        assert_eq!(
            execution
                .scene
                .component::<PipelineProgress>(content)
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation)),
            None
        );
        assert_eq!(
            execution
                .scene
                .component::<Translation>(translated_content)
                .unwrap()
                .unwrap()
                .text
                .value,
            "translation already saved"
        );
        assert!(
            execution
                .scene
                .component::<PipelineProgress>(translated_content)
                .unwrap()
                .unwrap()
                .is_complete(ProgressKind::Translation)
        );

        drop(execution);
        drop(committer);
        let snapshot = session.snapshot();
        let mut retry_committer = SessionCommitter::new(&mut session);
        let mut retry = Execution::new(
            runner.clone(),
            resources.clone(),
            snapshot,
            Request {
                operation: crate::Operation::Only {
                    stage: Stage::Translation,
                },
                ..Request::default()
            },
            &mut retry_committer,
        )
        .unwrap();
        assert!(retry.take_ready_job().is_some());
        let retry_input = StageInput::new(
            retry.scene.clone(),
            page,
            None,
            None,
            Arc::new(crate::images::ImageCache::default()),
            None,
        );
        assert!(
            !retry
                .runner
                .should_skip(Stage::Translation, &retry_input)
                .unwrap()
        );
        assert_eq!(
            retry
                .scene
                .component::<PipelineProgress>(page)
                .unwrap()
                .unwrap()
                .status(ProgressKind::Detection),
            Some(ProgressStatus::Complete(ProgressOrigin::Automatic))
        );
        drop(retry);
        drop(retry_committer);

        let stop = StopToken::default();
        stop.stop_with_reason("user_requested");
        let cancelled_snapshot = session.snapshot();
        let mut cancelled_committer = SessionCommitter::new(&mut session);
        let mut cancelled = Execution::new(
            runner,
            resources,
            cancelled_snapshot,
            Request {
                operation: crate::Operation::Only {
                    stage: Stage::Translation,
                },
                stop,
                ..Request::default()
            },
            &mut cancelled_committer,
        )
        .unwrap();
        cancelled
            .apply_completion(StageCompletion {
                page,
                stage: Stage::Translation,
                model: "mock".to_owned(),
                elapsed: Duration::ZERO,
                outcome: Err(PipelineError::new(
                    ErrorKind::Processing,
                    Some(Stage::Translation),
                    anyhow::anyhow!("simulated request cancelled while failing"),
                )),
            })
            .await
            .unwrap();
        let report = cancelled.finalize().unwrap();
        assert_eq!(report.status, RunStatus::Stopped);
        assert_eq!(
            cancelled_committer
                .session
                .snapshot()
                .component::<PipelineProgress>(content)
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation)),
            None
        );
    }

    #[tokio::test]
    async fn successful_translation_persists_translation_and_completion_marker() {
        let mut session = norrix_scene::Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(
                norrix_scene::PageDraft::new("translated", 100.0, 100.0),
                norrix_scene::At::End,
            )
            .unwrap();
        let content = edit.add_text_content(page, norrix_scene::At::End).unwrap();
        edit.set(
            content,
            &SourceText {
                text: norrix_scene::Authored::user("hello".to_owned()),
                language: None,
            },
        )
        .unwrap();
        edit.add_text_layer(
            page,
            norrix_scene::At::End,
            content,
            &norrix_scene::TextLayout {
                origin: norrix_scene::Origin::User,
                kind: norrix_scene::TextLayoutKind::Paragraph,
                angle_degrees: Some(0.0),
            },
        )
        .unwrap();
        session.commit(edit.finish().unwrap()).await.unwrap();

        let device = norrix_ml::Device::cpu();
        let resources = ResourceMonitor::new(&device);
        let translator = norrix_translator::Translator::from_config(
            device.clone(),
            norrix_config::Config::memory(norrix_translator::ProvidersConfig::default()),
        )
        .unwrap();
        let runner = Arc::new(
            StageRunner::new(
                &crate::PipelineConfig::default(),
                translator,
                &device,
                resources.clone(),
            )
            .unwrap(),
        );
        let commit_delay = Duration::from_millis(40);
        let mut committer = SessionCommitter::with_delay(&mut session, commit_delay);
        let mut execution = Execution::new(
            runner,
            resources,
            committer.session.snapshot(),
            Request {
                operation: crate::Operation::Only {
                    stage: Stage::Translation,
                },
                ..Request::default()
            },
            &mut committer,
        )
        .unwrap();
        let patch = execution
            .scene
            .patch(|edit| {
                edit.set(
                    content,
                    &Translation {
                        text: norrix_scene::Authored::user("hola".to_owned()),
                        language: Some(
                            norrix_scene::LanguageTag::new("es-ES").expect("valid language tag"),
                        ),
                    },
                )?;
                Ok(())
            })
            .unwrap();

        let commit_started = Instant::now();
        execution
            .apply_completion(StageCompletion {
                page,
                stage: Stage::Translation,
                model: "mock".to_owned(),
                elapsed: Duration::ZERO,
                outcome: Ok(StageOutcome::Patch(patch)),
            })
            .await
            .unwrap();
        let commit_and_marker_elapsed = commit_started.elapsed();

        let translated = execution
            .scene
            .component::<Translation>(content)
            .unwrap()
            .unwrap();
        assert_eq!(translated.text.value, "hola");
        assert!(
            execution
                .scene
                .component::<PipelineProgress>(content)
                .unwrap()
                .unwrap()
                .is_complete(ProgressKind::Translation)
        );
        drop(execution);
        assert_eq!(committer.commits, 2);
        assert!(commit_and_marker_elapsed >= commit_delay * 2);
    }

    #[tokio::test]
    async fn conflicting_translation_commit_does_not_mark_translation_complete() {
        let mut session = norrix_scene::Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(
                norrix_scene::PageDraft::new("translation conflict", 100.0, 100.0),
                norrix_scene::At::End,
            )
            .unwrap();
        let content = edit.add_text_content(page, norrix_scene::At::End).unwrap();
        edit.set(
            content,
            &norrix_scene::SourceText {
                text: norrix_scene::Authored::user("hello".to_owned()),
                language: None,
            },
        )
        .unwrap();
        edit.add_text_layer(
            page,
            norrix_scene::At::End,
            content,
            &norrix_scene::TextLayout {
                origin: norrix_scene::Origin::User,
                kind: norrix_scene::TextLayoutKind::Paragraph,
                angle_degrees: Some(0.0),
            },
        )
        .unwrap();
        session.commit(edit.finish().unwrap()).await.unwrap();

        let device = norrix_ml::Device::cpu();
        let resources = ResourceMonitor::new(&device);
        let translator = norrix_translator::Translator::from_config(
            device.clone(),
            norrix_config::Config::memory(norrix_translator::ProvidersConfig::default()),
        )
        .unwrap();
        let runner = Arc::new(
            StageRunner::new(
                &crate::PipelineConfig::default(),
                translator,
                &device,
                resources.clone(),
            )
            .unwrap(),
        );
        let mut committer = ConflictCommitter { commits: 0 };
        let mut execution = Execution::new(
            runner,
            resources,
            session.snapshot(),
            Request {
                operation: crate::Operation::Only {
                    stage: Stage::Translation,
                },
                ..Request::default()
            },
            &mut committer,
        )
        .unwrap();
        let patch = execution
            .scene
            .patch(|edit| {
                edit.set(
                    content,
                    &Translation {
                        text: norrix_scene::Authored::user("hola".to_owned()),
                        language: Some(
                            norrix_scene::LanguageTag::new("es-ES").expect("valid language tag"),
                        ),
                    },
                )?;
                Ok(())
            })
            .unwrap();

        assert!(
            execution
                .apply_completion(StageCompletion {
                    page,
                    stage: Stage::Translation,
                    model: "mock".to_owned(),
                    elapsed: Duration::ZERO,
                    outcome: Ok(StageOutcome::Patch(patch)),
                })
                .await
                .is_err()
        );
        assert!(
            execution
                .scene
                .component::<Translation>(content)
                .unwrap()
                .is_none()
        );
        assert!(
            execution
                .scene
                .component::<PipelineProgress>(content)
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation))
                .is_none()
        );
        drop(execution);
        assert_eq!(committer.commits, 1);
    }
}
