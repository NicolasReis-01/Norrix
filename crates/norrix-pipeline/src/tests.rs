use super::*;

#[test]
fn configuration_ignores_unknown_fields() {
    let config = toml::from_str::<PipelineConfig>("legacy_limit = 1").unwrap();
    assert_eq!(config, PipelineConfig::default());
}

#[tokio::test]
async fn project_translation_scan_skips_completed_and_empty_pages() {
    let pipeline = pipeline(TranslationConfig::default());
    let mut session = norrix_scene::Session::memory().await.unwrap();
    let mut edit = session.snapshot().edit();
    let completed_page = edit
        .add_page(
            norrix_scene::PageDraft::new("completed", 100.0, 100.0),
            norrix_scene::At::End,
        )
        .unwrap();
    let content = edit
        .add_text_content(completed_page, norrix_scene::At::End)
        .unwrap();
    edit.set(
        content,
        &norrix_scene::SourceText {
            text: norrix_scene::Authored::user("source".to_owned()),
            language: None,
        },
    )
    .unwrap();
    edit.set(
        content,
        &norrix_scene::Translation {
            text: norrix_scene::Authored::user("translated".to_owned()),
            language: None,
        },
    )
    .unwrap();
    let mut progress = norrix_scene::PipelineProgress::default();
    progress.set(
        norrix_scene::ProgressKind::Translation,
        Some(norrix_scene::ProgressStatus::Complete(
            norrix_scene::ProgressOrigin::Automatic,
        )),
    );
    edit.set(content, &progress).unwrap();
    edit.add_text_layer(
        completed_page,
        norrix_scene::At::End,
        content,
        &norrix_scene::TextLayout {
            origin: norrix_scene::Origin::User,
            kind: norrix_scene::TextLayoutKind::Paragraph,
            angle_degrees: None,
        },
    )
    .unwrap();
    edit.add_page(
        norrix_scene::PageDraft::new("empty", 100.0, 100.0),
        norrix_scene::At::End,
    )
    .unwrap();
    session.commit(edit.finish().unwrap()).await.unwrap();
    let mut committer = RejectCommitter;
    let skipped = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skipped_progress = skipped.clone();

    let report = pipeline
        .execute(
            session.snapshot(),
            Request {
                operation: Operation::Only {
                    stage: Stage::Translation,
                },
                scope: Scope::Project,
                progress: Some(std::sync::Arc::new(move |event| {
                    if matches!(event, Progress::Skipped { .. }) {
                        skipped_progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                })),
                ..Request::default()
            },
            &mut committer,
        )
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.total, 2);
    assert_eq!(skipped.load(std::sync::atomic::Ordering::Relaxed), 2);
    let snapshot = session.snapshot();
    assert_eq!(
        snapshot
            .component::<norrix_scene::Translation>(content)
            .unwrap()
            .unwrap()
            .text
            .value,
        "translated"
    );
    assert!(
        snapshot
            .component::<norrix_scene::PipelineProgress>(content)
            .unwrap()
            .unwrap()
            .is_complete(norrix_scene::ProgressKind::Translation)
    );
}

#[tokio::test]
async fn stop_is_a_successful_partial_result() {
    let pipeline = pipeline(Default::default());
    let stop = StopToken::default();
    stop.stop();
    let request = Request {
        stop,
        ..Request::default()
    };
    let mut committer = RejectCommitter;

    let report = pipeline
        .execute(
            norrix_scene::Session::memory().await.unwrap().snapshot(),
            request,
            &mut committer,
        )
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Stopped);
    assert_eq!(report.completed, 0);
}

#[tokio::test]
async fn stop_after_a_page_keeps_completed_progress() {
    let translation = TranslationConfig {
        model: norrix_translator::ModelSelection {
            provider: norrix_translator::Provider::OpenAi,
            model: Some("gpt-5.6-luna".to_owned()),
            quantization: None,
            vision: true,
            reasoning: true,
        },
        ..Default::default()
    };
    let pipeline = pipeline(translation);
    let mut session = norrix_scene::Session::memory().await.unwrap();
    let patch = session
        .snapshot()
        .patch(|edit| {
            edit.add_page(
                norrix_scene::PageDraft::new("one", 1.0, 1.0),
                norrix_scene::At::End,
            )?;
            edit.add_page(
                norrix_scene::PageDraft::new("two", 1.0, 1.0),
                norrix_scene::At::End,
            )?;
            Ok(())
        })
        .unwrap();
    session.commit(patch).await.unwrap();
    let stop = StopToken::default();
    let progress_stop = stop.clone();
    let request = Request {
        operation: Operation::Only {
            stage: Stage::Translation,
        },
        stop,
        progress: Some(std::sync::Arc::new(move |event| {
            if matches!(event, Progress::Skipped { .. }) {
                progress_stop.stop();
            }
        })),
        ..Request::default()
    };
    let mut committer = RejectCommitter;

    let report = pipeline
        .execute(session.snapshot(), request, &mut committer)
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Stopped);
    assert_eq!(report.completed, 1);
    assert_eq!(report.total, 2);
}

#[tokio::test]
async fn completed_page_and_text_markers_resume_without_loading_models() {
    let pipeline = pipeline(TranslationConfig::default());
    let mut session = norrix_scene::Session::memory().await.unwrap();
    let mut edit = session.snapshot().edit();
    let page = edit
        .add_page(
            norrix_scene::PageDraft::new("processed", 100.0, 100.0),
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
    let mut page_progress = norrix_scene::PipelineProgress::default();
    for kind in [
        norrix_scene::ProgressKind::Detection,
        norrix_scene::ProgressKind::Ocr,
        norrix_scene::ProgressKind::Inpainting,
    ] {
        page_progress.set(
            kind,
            Some(norrix_scene::ProgressStatus::Complete(
                norrix_scene::ProgressOrigin::Manual,
            )),
        );
    }
    edit.set(page, &page_progress).unwrap();
    let mut text_progress = norrix_scene::PipelineProgress::default();
    text_progress.set(
        norrix_scene::ProgressKind::Translation,
        Some(norrix_scene::ProgressStatus::Complete(
            norrix_scene::ProgressOrigin::Manual,
        )),
    );
    edit.set(content, &text_progress).unwrap();
    session.commit(edit.finish().unwrap()).await.unwrap();
    let mut committer = RejectCommitter;

    let report = pipeline
        .execute(
            session.snapshot(),
            Request {
                operation: Operation::Full,
                ..Request::default()
            },
            &mut committer,
        )
        .await
        .unwrap();

    assert_eq!(report.status, RunStatus::Completed);
    assert_eq!(report.completed, 4);
    assert_eq!(report.total, 4);
}

struct RejectCommitter;

fn pipeline(translation: TranslationConfig) -> Pipeline {
    let config = PipelineConfig {
        translation,
        ..PipelineConfig::default()
    };
    Pipeline::from_config(
        norrix_config::Config::memory(config),
        norrix_config::Config::memory(norrix_translator::ProvidersConfig::default()),
        norrix_ml::Device::cpu(),
    )
    .unwrap()
}

#[async_trait::async_trait]
impl Committer for RejectCommitter {
    async fn commit(&mut self, _output: StageOutput) -> anyhow::Result<CommitResult> {
        anyhow::bail!("stopped execution must not commit")
    }
}

#[test]
fn operations_expand_to_the_supported_workflows() {
    assert_eq!(
        Operation::Through {
            stage: Stage::Translation,
        }
        .stages()
        .unwrap(),
        vec![Stage::Detection, Stage::Ocr, Stage::Translation],
    );
    assert_eq!(
        Operation::Through {
            stage: Stage::Inpainting,
        }
        .stages()
        .unwrap(),
        vec![Stage::Detection, Stage::Inpainting],
    );
    assert_eq!(
        Operation::Only {
            stage: Stage::Translation,
        }
        .stages()
        .unwrap(),
        vec![Stage::Translation],
    );
    assert_eq!(
        Operation::Stages {
            stages: vec![Stage::Translation, Stage::Detection, Stage::Translation],
        }
        .stages()
        .unwrap(),
        vec![Stage::Detection, Stage::Translation],
    );
}
