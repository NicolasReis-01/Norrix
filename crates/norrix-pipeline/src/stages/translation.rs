use anyhow::Result;
use async_trait::async_trait;
use image::GrayImage;
use norrix_scene::{
    AssetRole, Authored, EntityId, EntityOrigin, Geometry, LanguageTag, OcrAnalysis, Origin,
    OriginalFontEstimate, RecognizedFrom, Region, RegionSpec, SourceText, TextDirection,
    TextLayout, TextRegion, Translation, Typography, WritingMode,
};
use norrix_translator::{TranslationRequest, Translator};
use std::{collections::BTreeMap, time::Instant};
use tracing::Instrument;

use crate::TranslationConfig;

use super::{StageInput, StageProcessor, finish, generation};

const PRODUCER: &str = "dev.koharu.pipeline.translation";

pub(super) struct Processor {
    config: TranslationConfig,
    translator: Translator,
    #[cfg(test)]
    mock_translation_result: Option<(&'static str, Vec<String>)>,
}

fn valid_translation_box(input: &StageInput, layer: EntityId, content: EntityId) -> Result<bool> {
    if !input.contains_entity(layer)? {
        return Ok(false);
    }
    match input.scene.component::<EntityOrigin>(layer)? {
        Some(origin) if matches!(origin.origin, Origin::User) => return Ok(true),
        _ => {}
    }
    match input.scene.component::<TextLayout>(layer)? {
        Some(layout) if matches!(layout.origin, Origin::User) => return Ok(true),
        Some(layout) if matches!(layout.origin, Origin::Generated(_)) => {}
        Some(_) => {}
        None => return Ok(true),
    }
    let Some(relation) = input.scene.relation_from::<RecognizedFrom>(content)? else {
        return Ok(false);
    };
    let region = relation.value().target;
    Ok(input
        .scene
        .component::<Region>(region)?
        .is_some_and(|value| value.kind == TextRegion::kind())
        && input.contains_entity(region)?)
}

impl Processor {
    pub(super) fn new(config: TranslationConfig, translator: Translator) -> Self {
        Self {
            config,
            translator,
            #[cfg(test)]
            mock_translation_result: None,
        }
    }

    async fn translate_request(
        &self,
        request: TranslationRequest,
    ) -> anyhow::Result<(&'static str, Vec<String>)> {
        #[cfg(test)]
        if let Some(result) = &self.mock_translation_result {
            return Ok(result.clone());
        }
        self.translator
            .translate(&self.config.model, self.config.generation, request)
            .await
    }
}

#[async_trait]
impl StageProcessor for Processor {
    fn model(&self) -> &'static str {
        Translator::model(&self.config.model)
    }

    fn unload(&self) -> bool {
        self.translator.unload()
    }

    fn skip(&self, input: &StageInput) -> Result<bool> {
        let mut persisted_box_count = 0usize;
        let mut eligible_segment_count = 0usize;
        let mut pending_segment_count = 0usize;
        let mut already_translated_count = 0usize;
        let Some(group) = input.scene.page(input.page)?.text_group()? else {
            tracing::info!(
                event = "translation_segments_selected",
                page = %input.page,
                persisted_box_count,
                eligible_segment_count,
                pending_segment_count,
                already_translated_count,
                translation_skipped = true,
                skip_reason = "no_text_group",
                provider_request_started = false,
                trd_state = "0/0",
                result = "skipped",
            );
            return Ok(true);
        };
        for layer in group.text_layers()? {
            if !valid_translation_box(input, layer.id(), layer.content()?.id())? {
                continue;
            }
            persisted_box_count += 1;
            let content = layer.content()?;
            let Some(source) = content.source()? else {
                continue;
            };
            if source.text.value.trim().is_empty() {
                continue;
            }
            eligible_segment_count += 1;
            if input.translation_complete(content.id())? {
                already_translated_count += 1;
            } else {
                pending_segment_count += 1;
            }
        }
        let skipped = pending_segment_count == 0;
        tracing::info!(
            event = "translation_segments_selected",
            page = %input.page,
            persisted_box_count,
            eligible_segment_count,
            pending_segment_count,
            already_translated_count,
            translation_skipped = skipped,
            skip_reason = if persisted_box_count == 0 { "no_confirmed_text_boxes" } else if pending_segment_count == 0 { "all_segments_complete" } else { "pending_segments" },
            provider_request_started = false,
            trd_state = if persisted_box_count == 0 { "0/0" } else { "pending_or_partial" },
            result = if skipped { "skipped" } else { "selected" },
        );
        Ok(skipped)
    }

    async fn load(&self) -> Result<()> {
        self.translator.load_model(&self.config.model).await
    }

    async fn process(&self, input: StageInput) -> Result<norrix_scene::Patch> {
        let provider = self.config.model.provider;
        let model = self
            .config
            .model
            .model
            .as_deref()
            .unwrap_or("provider_default");
        let selection_started = Instant::now();
        let mut targets = Vec::new();
        let mut persisted_box_count = 0usize;
        let mut eligible_segment_count = 0usize;
        let mut already_translated_count = 0usize;
        if let Some(group) = input.scene.page(input.page)?.text_group()? {
            for layer in group.text_layers()? {
                let content = layer.content()?;
                if !valid_translation_box(&input, layer.id(), content.id())? {
                    continue;
                }
                persisted_box_count += 1;
                let Some(source) = content.source()? else {
                    continue;
                };
                if source.text.value.trim().is_empty() {
                    continue;
                }
                eligible_segment_count += 1;
                if input.translation_complete(content.id())? {
                    already_translated_count += 1;
                } else {
                    targets.push((content.id(), layer.id(), source.text.value));
                }
            }
        }
        tracing::info!(
            event = "translation_segments_selected",
            page = %input.page,
            provider = %provider,
            model,
            target_language = %self.config.target_language,
            segment_count = targets.len(),
            persisted_box_count,
            eligible_segment_count,
            pending_segment_count = targets.len(),
            already_translated_count,
            translation_skipped = targets.is_empty(),
            skip_reason = if targets.is_empty() { "no_pending_segments" } else { "pending_segments" },
            provider_request_started = !targets.is_empty(),
            duration_ms = selection_started.elapsed().as_secs_f64() * 1000.0,
            result = if targets.is_empty() { "skipped" } else { "selected" },
        );

        if targets.is_empty() {
            return finish(input.scene.edit());
        }

        let image_started = Instant::now();
        let image_required =
            Translator::supports_vision(&self.config.model, &self.config.generation);
        let image_result = if image_required {
            input.images.get(&input.scene, input.page, "source").await
        } else {
            Ok(None)
        };
        let image = match image_result {
            Ok(image) => {
                tracing::info!(
                    event = "translation_source_image_load_finished",
                    page = %input.page,
                    provider = %provider,
                    model,
                    duration_ms = image_started.elapsed().as_secs_f64() * 1000.0,
                    result = if image.is_some() {
                        "image_ready"
                    } else if image_required {
                        "image_not_available"
                    } else {
                        "not_required"
                    },
                );
                image
            }
            Err(error) => {
                tracing::error!(
                    event = "translation_source_image_load_failed",
                    page = %input.page,
                    provider = %provider,
                    model,
                    duration_ms = image_started.elapsed().as_secs_f64() * 1000.0,
                    result = "failed",
                );
                return Err(error.into());
            }
        };

        let request_started = Instant::now();
        let mut request = TranslationRequest::new(
            targets.iter().map(|(_, _, source)| source.clone()),
            self.config.target_language,
        );
        if let Some(instructions) = self.config.instructions.as_deref() {
            request = request.with_instructions(instructions);
        }
        if let Some(image) = image {
            request = request.with_image(image);
        }
        tracing::info!(
            event = "translation_request_constructed",
            page = %input.page,
            provider = %provider,
            model,
            target_language = %self.config.target_language,
            segment_count = targets.len(),
            duration_ms = request_started.elapsed().as_secs_f64() * 1000.0,
            result = "constructed",
        );

        let started = Instant::now();
        tracing::info!(
            event = "translation_provider_request_started",
            page = %input.page,
            provider = %provider,
            model,
            segment_count = targets.len(),
            provider_request_started = true,
            result = "started",
        );
        let provider_span = tracing::info_span!(
            "translation_stage_provider_call",
            page = %input.page,
            provider = %provider,
            model,
        );
        let translation = self
            .translate_request(request)
            .instrument(provider_span)
            .await;
        let (provider, translated) = match translation {
            Ok(result) => {
                tracing::info!(
                    event = "translation_provider_finished",
                    page = %input.page,
                    provider = result.0,
                    model,
                    target_language = %self.config.target_language,
                    segment_count = targets.len(),
                    provider_result = "success",
                    translated_segment_count = targets.len(),
                    provider_end_to_end_duration_ms =
                        started.elapsed().as_secs_f64() * 1000.0,
                    result = "translated",
                );
                result
            }
            Err(error) => {
                tracing::error!(
                    event = "translation_provider_failed",
                    page = %input.page,
                    provider = %provider,
                    model,
                    target_language = %self.config.target_language,
                    segment_count = targets.len(),
                    provider_end_to_end_duration_ms =
                        started.elapsed().as_secs_f64() * 1000.0,
                    result = "failed",
                );
                return Err(error.into());
            }
        };
        let patch_translation_count = translated.len();
        let font_size_estimates =
            estimate_target_font_sizes_best_effort(&input.scene, input.page, &targets).await;
        let patch_started = Instant::now();
        let patch = build_translation_patch(
            &input.scene,
            targets,
            translated,
            &font_size_estimates,
            provider,
            self.config.target_language,
        );
        match patch {
            Ok(patch) => {
                tracing::info!(
                    event = "translation_patch_constructed",
                    page = %input.page,
                    provider,
                    model,
                    duration_ms = patch_started.elapsed().as_secs_f64() * 1000.0,
                    patch_translation_count,
                    persisted = false,
                    result = "constructed",
                );
                tracing::info!(
                    event = "translation_stage_processing_finished",
                    page = %input.page,
                    provider,
                    model,
                    target_language = %self.config.target_language,
                    duration_ms = selection_started.elapsed().as_secs_f64() * 1000.0,
                    result = "patch_ready",
                );
                Ok(patch)
            }
            Err(error) => {
                tracing::error!(
                    event = "translation_patch_construction_failed",
                    page = %input.page,
                    provider,
                    model,
                    duration_ms = patch_started.elapsed().as_secs_f64() * 1000.0,
                    result = "failed",
                );
                Err(error)
            }
        }
    }
}

async fn estimate_target_font_sizes_best_effort(
    scene: &norrix_scene::Snapshot,
    page: EntityId,
    targets: &[(EntityId, EntityId, String)],
) -> BTreeMap<EntityId, f32> {
    let result = async {
        let Some(mask) = load_text_mask(scene, page).await? else {
            return Ok(BTreeMap::new());
        };
        estimate_target_font_sizes(scene, page, targets, &mask)
    }
    .await;
    match result {
        Ok(estimates) => estimates,
        Err(error) => {
            tracing::warn!(
                event = "translation_original_font_estimate_unavailable",
                page = %page,
                reason = "estimation_failed",
                result = "translation_continues_without_estimate",
                error = %error,
            );
            BTreeMap::new()
        }
    }
}

fn build_translation_patch(
    scene: &norrix_scene::Snapshot,
    targets: Vec<(EntityId, EntityId, String)>,
    translated: Vec<String>,
    font_size_estimates: &BTreeMap<EntityId, f32>,
    provider: &'static str,
    target_language: norrix_translator::Language,
) -> Result<norrix_scene::Patch> {
    anyhow::ensure!(
        translated.len() == targets.len(),
        "translation provider returned {} segments for {} targets",
        translated.len(),
        targets.len()
    );
    for (index, translation) in translated.iter().enumerate() {
        anyhow::ensure!(
            !translation.trim().is_empty(),
            "translation provider returned an empty translation for segment {index}"
        );
    }
    let language = LanguageTag::new(target_language.tag())?;
    let generated = generation(PRODUCER, provider)?;
    let mut edit = scene.edit_as(generated.clone());
    for (entity, _, _) in &targets {
        edit.observe::<SourceText>(*entity)?;
        edit.observe::<Translation>(*entity)?;
    }
    for ((entity, layer, source), text) in targets.into_iter().zip(translated) {
        let text = if source.trim() == "\u{2026}" {
            "\u{2026}".to_owned()
        } else {
            text
        };
        edit.set(
            entity,
            &Translation {
                text: Authored::generated(text, generated.clone()),
                language: Some(language.clone()),
            },
        )?;
        if let Some(size) = font_size_estimates.get(&layer) {
            match set_original_font_estimate(scene, &mut edit, layer, *size, &generated) {
                Ok(true) => {}
                Ok(false) => {}
                Err(error) => tracing::warn!(
                    event = "translation_original_font_estimate_persistence_failed",
                    layer = %layer,
                    reason = "estimate_not_persisted",
                    result = "translation_continues",
                    error = %error,
                ),
            }
        }
    }
    finish(edit)
}

fn set_original_font_estimate(
    scene: &norrix_scene::Snapshot,
    edit: &mut norrix_scene::Edit,
    layer: EntityId,
    size: f32,
    generated: &norrix_scene::Generation,
) -> Result<bool> {
    if !size.is_finite() || size <= 0.0 {
        return Ok(false);
    }
    let Some(typography) = scene.component::<Typography>(layer)? else {
        return Ok(false);
    };
    if !matches!(typography.origin, Origin::Generated(_)) || typography.size.is_some() {
        return Ok(false);
    }
    if scene
        .component::<OriginalFontEstimate>(layer)?
        .is_some_and(|estimate| {
            !matches!(
                estimate.origin,
                Origin::Generated(ref owner) if owner.producer == generated.producer
            )
        })
    {
        return Ok(false);
    }
    edit.set(
        layer,
        &OriginalFontEstimate {
            origin: Origin::Generated(generated.clone()),
            size,
        },
    )?;
    Ok(true)
}

async fn load_text_mask(
    scene: &norrix_scene::Snapshot,
    page: EntityId,
) -> Result<Option<GrayImage>> {
    let role = AssetRole::new("text-mask")?;
    let Some(asset) = scene.asset(page, &role)? else {
        return Ok(None);
    };
    let bytes = scene.read_blob(asset.blob).await?;
    match image::load_from_memory(&bytes) {
        Ok(image) => Ok(Some(image.to_luma8())),
        Err(error) => {
            tracing::warn!(
                event = "translation_original_font_estimate_unavailable",
                page = %page,
                reason = "invalid_text_mask",
                error = %error,
            );
            Ok(None)
        }
    }
}

fn estimate_target_font_sizes(
    scene: &norrix_scene::Snapshot,
    page: EntityId,
    targets: &[(EntityId, EntityId, String)],
    mask: &GrayImage,
) -> Result<BTreeMap<EntityId, f32>> {
    let page_size = scene.page(page)?.page()?;
    if page_size.width <= 0.0 || page_size.height <= 0.0 {
        return Ok(BTreeMap::new());
    }
    let mut estimates = BTreeMap::new();
    for (content, layer, _) in targets {
        let Some(typography) = scene.component::<Typography>(*layer)? else {
            continue;
        };
        if !matches!(typography.origin, Origin::Generated(_)) || typography.size.is_some() {
            continue;
        }
        let Some(relation) = scene.relation_from::<RecognizedFrom>(*content)? else {
            continue;
        };
        let region = relation.value().target;
        let Some(geometry) = scene.component::<Geometry>(region)? else {
            continue;
        };
        let Some(bounds) = geometry_bounds(&geometry) else {
            continue;
        };
        let direction = typography
            .writing_mode
            .map(|mode| match mode {
                WritingMode::Horizontal => TextDirection::Horizontal,
                WritingMode::Vertical => TextDirection::Vertical,
            })
            .or(scene
                .component::<OcrAnalysis>(region)?
                .map(|analysis| analysis.direction))
            .unwrap_or(TextDirection::Auto);
        let vertical = match direction {
            TextDirection::Vertical => true,
            TextDirection::Horizontal => false,
            TextDirection::Auto => bounds.3 - bounds.1 > bounds.2 - bounds.0,
        };
        if let Some(size) = estimate_font_size_in_region(
            mask,
            bounds,
            (page_size.width, page_size.height),
            vertical,
        ) {
            estimates.insert(*layer, size);
        }
    }
    Ok(estimates)
}

fn geometry_bounds(geometry: &Geometry) -> Option<(f64, f64, f64, f64)> {
    let first = *geometry.points.first()?;
    let bounds = geometry.points.iter().skip(1).fold(
        (first.x, first.y, first.x, first.y),
        |(left, top, right, bottom), point| {
            (
                left.min(point.x),
                top.min(point.y),
                right.max(point.x),
                bottom.max(point.y),
            )
        },
    );
    (bounds.2 > bounds.0 && bounds.3 > bounds.1).then_some(bounds)
}

fn estimate_font_size_in_region(
    mask: &GrayImage,
    bounds: (f64, f64, f64, f64),
    page_size: (f64, f64),
    vertical: bool,
) -> Option<f32> {
    let scale_x = f64::from(mask.width()) / page_size.0;
    let scale_y = f64::from(mask.height()) / page_size.1;
    let left = (bounds.0 * scale_x)
        .floor()
        .clamp(0.0, f64::from(mask.width())) as u32;
    let top = (bounds.1 * scale_y)
        .floor()
        .clamp(0.0, f64::from(mask.height())) as u32;
    let right = (bounds.2 * scale_x)
        .ceil()
        .clamp(0.0, f64::from(mask.width())) as u32;
    let bottom = (bounds.3 * scale_y)
        .ceil()
        .clamp(0.0, f64::from(mask.height())) as u32;
    if right <= left || bottom <= top {
        return None;
    }
    let run_sizes = projection_runs(mask, left, top, right, bottom, vertical);
    let mut run_sizes = run_sizes
        .into_iter()
        .filter(|size| *size >= 2)
        .collect::<Vec<_>>();
    if run_sizes.is_empty() {
        return None;
    }
    run_sizes.sort_unstable();
    let glyph_extent = f64::from(run_sizes[run_sizes.len() / 2]);
    let page_units_per_pixel = if vertical {
        1.0 / scale_x
    } else {
        1.0 / scale_y
    };
    let em_factor = if vertical { 1.0 } else { 1.3 };
    let estimated = glyph_extent * page_units_per_pixel * em_factor;
    let region_extent = if vertical {
        bounds.2 - bounds.0
    } else {
        bounds.3 - bounds.1
    };
    let estimated = estimated.min(region_extent).max(0.0);
    (estimated.is_finite() && estimated >= 1.0).then_some(estimated as f32)
}

fn projection_runs(
    mask: &GrayImage,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    vertical: bool,
) -> Vec<u32> {
    let (length, cross_length) = if vertical {
        (right - left, bottom - top)
    } else {
        (bottom - top, right - left)
    };
    let minimum_ink = (cross_length / 200).max(1);
    let mut active = Vec::with_capacity(length as usize);
    for offset in 0..length {
        let count = if vertical {
            (top..bottom)
                .filter(|y| mask.get_pixel(left + offset, *y).0[0] > 0)
                .count() as u32
        } else {
            (left..right)
                .filter(|x| mask.get_pixel(*x, top + offset).0[0] > 0)
                .count() as u32
        };
        active.push(count >= minimum_ink);
    }

    let mut runs = Vec::new();
    let mut start = None;
    let mut inactive_gap = 0;
    for (index, is_active) in active.iter().copied().enumerate() {
        if is_active {
            if start.is_none() {
                start = Some(index);
            }
            inactive_gap = 0;
        } else if start.is_some() {
            inactive_gap += 1;
            if inactive_gap > 1 {
                let end = index - inactive_gap + 1;
                runs.push((end - start.take().unwrap_or(end)) as u32);
                inactive_gap = 0;
            }
        }
    }
    if let Some(start) = start {
        runs.push((active.len() - start) as u32);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;
    use norrix_scene::{
        At, Generation, PageDraft, PipelineProgress, ProgressKind, Session, TextLayout,
        TextLayoutKind,
    };

    #[tokio::test]
    async fn skips_translation_when_there_are_no_eligible_segments() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(PageDraft::new("empty translation", 100.0, 100.0), At::End)
            .unwrap();
        let content = edit.add_text_content(page, At::End).unwrap();
        edit.set(
            content,
            &SourceText {
                text: Authored::user("   ".to_owned()),
                language: None,
            },
        )
        .unwrap();
        edit.add_text_layer(
            page,
            At::End,
            content,
            &TextLayout {
                origin: Origin::User,
                kind: TextLayoutKind::Paragraph,
                angle_degrees: Some(0.0),
            },
        )
        .unwrap();
        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;
        let translator = Translator::from_config(
            norrix_ml::Device::cpu(),
            norrix_config::Config::memory(norrix_translator::ProvidersConfig::default()),
        )
        .unwrap();
        let processor = Processor::new(TranslationConfig::default(), translator);
        let input = StageInput::new(
            snapshot,
            page,
            None,
            None,
            std::sync::Arc::new(crate::ImageCache::default()),
            None,
        );

        assert!(processor.skip(&input).unwrap());
    }

    #[test]
    fn original_font_estimate_tracks_glyph_height_and_vertical_glyph_width() {
        let bounds = (10.0, 10.0, 90.0, 90.0);
        let mut small = GrayImage::new(100, 100);
        for y in 35..43 {
            for x in 20..80 {
                small.put_pixel(x, y, Luma([255]));
            }
        }
        let mut large = GrayImage::new(100, 100);
        for y in 30..55 {
            for x in 20..80 {
                large.put_pixel(x, y, Luma([255]));
            }
        }
        let small_size =
            estimate_font_size_in_region(&small, bounds, (100.0, 100.0), false).unwrap();
        let large_size =
            estimate_font_size_in_region(&large, bounds, (100.0, 100.0), false).unwrap();
        assert!(large_size > small_size * 2.0);

        let mut vertical = GrayImage::new(100, 100);
        for y in 20..80 {
            for x in 46..54 {
                vertical.put_pixel(x, y, Luma([255]));
            }
        }
        let vertical_size =
            estimate_font_size_in_region(&vertical, (45.0, 10.0, 55.0, 90.0), (100.0, 100.0), true)
                .unwrap();
        assert!(vertical_size >= 7.0);
        assert!(vertical_size < 10.0);
    }

    #[tokio::test]
    async fn generated_translation_patch_persists_without_writing_progress() {
        let mut session = Session::memory().await.unwrap();
        let snapshot = session.snapshot();
        let mut edit = snapshot.edit();
        let page = edit
            .add_page(PageDraft::new("translation", 100.0, 100.0), At::End)
            .unwrap();
        let content = edit.add_text_content(page, At::End).unwrap();
        edit.set(
            content,
            &SourceText {
                text: Authored::user("hello".to_owned()),
                language: None,
            },
        )
        .unwrap();
        let layer = edit
            .add_text_layer(
                page,
                At::End,
                content,
                &TextLayout {
                    origin: norrix_scene::Origin::User,
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: Some(0.0),
                },
            )
            .unwrap();
        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;

        let patch = build_translation_patch(
            &snapshot,
            vec![(content, layer, "hello".to_owned())],
            vec!["hola".to_owned()],
            &BTreeMap::new(),
            "test-provider",
            norrix_translator::Language::Spanish,
        )
        .unwrap();
        let translated = session.commit(patch).await.unwrap().snapshot;

        assert_eq!(
            translated
                .component::<Translation>(content)
                .unwrap()
                .unwrap()
                .text
                .value,
            "hola"
        );
        assert!(
            translated
                .component::<PipelineProgress>(content)
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation))
                .is_none()
        );
    }

    #[tokio::test]
    async fn translation_persists_when_font_estimation_fails() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(PageDraft::new("estimation failure", 100.0, 100.0), At::End)
            .unwrap();
        let content = edit.add_text_content(page, At::End).unwrap();
        edit.set(
            content,
            &SourceText {
                text: Authored::user("hello".to_owned()),
                language: None,
            },
        )
        .unwrap();
        let layer = edit
            .add_text_layer(
                page,
                At::End,
                content,
                &TextLayout {
                    origin: Origin::User,
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: None,
                },
            )
            .unwrap();
        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;
        let targets = vec![(content, layer, "hello".to_owned())];

        let estimates =
            estimate_target_font_sizes_best_effort(&snapshot, EntityId::new(), &targets).await;
        assert!(estimates.is_empty());

        let request = TranslationRequest::new(
            targets.iter().map(|(_, _, source)| source.clone()),
            norrix_translator::Language::Spanish,
        );
        assert_eq!(request.segments, ["hello"]);
        let patch = build_translation_patch(
            &snapshot,
            targets,
            vec!["hola".to_owned()],
            &BTreeMap::from([(layer, f32::NAN)]),
            "test-provider",
            norrix_translator::Language::Spanish,
        )
        .unwrap();
        let translated = session.commit(patch).await.unwrap().snapshot;

        assert_eq!(
            translated
                .component::<Translation>(content)
                .unwrap()
                .unwrap()
                .text
                .value,
            "hola"
        );
        assert!(
            translated
                .component::<OriginalFontEstimate>(layer)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn mocked_provider_translation_runs_from_selection_through_persistence() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(PageDraft::new("mocked translation", 100.0, 100.0), At::End)
            .unwrap();
        let content = edit.add_text_content(page, At::End).unwrap();
        edit.set(
            content,
            &SourceText {
                text: Authored::user("hello".to_owned()),
                language: None,
            },
        )
        .unwrap();
        let layer = edit
            .add_text_layer(
                page,
                At::End,
                content,
                &TextLayout {
                    origin: Origin::User,
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: None,
                },
            )
            .unwrap();
        edit.set(layer, &Geometry::rectangle(5.0, 5.0, 80.0, 30.0))
            .unwrap();
        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;

        let device = norrix_ml::Device::cpu();
        let translator = Translator::from_config(
            device,
            norrix_config::Config::memory(norrix_translator::ProvidersConfig::default()),
        )
        .unwrap();
        let mut processor = Processor::new(TranslationConfig::default(), translator);
        processor.mock_translation_result = Some(("test-provider", vec!["hola".to_owned()]));
        let input = StageInput::new(
            snapshot,
            page,
            None,
            None,
            std::sync::Arc::new(crate::ImageCache::default()),
            None,
        );

        assert!(!processor.skip(&input).unwrap());
        let patch = processor.process(input).await.unwrap();
        let translated = session.commit(patch).await.unwrap().snapshot;

        assert_eq!(
            translated
                .component::<Translation>(content)
                .unwrap()
                .unwrap()
                .text
                .value,
            "hola"
        );
        let rendered = norrix_renderer::Renderer::default()
            .render(&translated, page)
            .await
            .unwrap();
        let layer = rendered
            .layer(layer)
            .expect("persisted translation should produce a rendered text layer");
        let norrix_renderer::LayerKind::Text(metadata) = layer.kind() else {
            panic!("translated layer should render as text");
        };
        assert_eq!(metadata.text, "hola");
    }

    #[tokio::test]
    async fn foreign_font_estimate_does_not_block_translation_persistence() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(PageDraft::new("foreign estimate", 100.0, 100.0), At::End)
            .unwrap();
        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;

        let existing_generation = Generation::new(
            norrix_scene::ProducerId::new("dev.norrix.pipeline.legacy-translation").unwrap(),
        );
        let mut edit = snapshot.edit_as(existing_generation.clone());
        let content = edit.add_text_content(page, At::End).unwrap();
        edit.set(
            content,
            &SourceText {
                text: Authored::generated("hello".to_owned(), existing_generation.clone()),
                language: None,
            },
        )
        .unwrap();
        let layer = edit
            .add_text_layer(
                page,
                At::End,
                content,
                &TextLayout {
                    origin: Origin::Generated(existing_generation.clone()),
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: None,
                },
            )
            .unwrap();
        edit.set(
            layer,
            &Typography {
                origin: Origin::Generated(existing_generation.clone()),
                preferred_font: None,
                font_weight: None,
                font_style: None,
                size: None,
                auto_fit: true,
                auto_fit_size: None,
                color: None,
                stroke_color: None,
                stroke_width: None,
                alignment: None,
                writing_mode: None,
                line_height: None,
                letter_spacing: None,
                extensions: Default::default(),
            },
        )
        .unwrap();
        edit.set(
            layer,
            &OriginalFontEstimate {
                origin: Origin::Generated(existing_generation),
                size: 12.0,
            },
        )
        .unwrap();
        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;

        let patch = build_translation_patch(
            &snapshot,
            vec![(content, layer, "hello".to_owned())],
            vec!["hola".to_owned()],
            &BTreeMap::from([(layer, 20.0)]),
            "test-provider",
            norrix_translator::Language::Spanish,
        )
        .unwrap();
        let translated = session.commit(patch).await.unwrap().snapshot;

        assert_eq!(
            translated
                .component::<Translation>(content)
                .unwrap()
                .unwrap()
                .text
                .value,
            "hola"
        );
        assert_eq!(
            translated
                .component::<OriginalFontEstimate>(layer)
                .unwrap()
                .unwrap()
                .size,
            12.0
        );
    }

    #[tokio::test]
    async fn translation_patch_persists_estimate_and_preserves_manual_typography() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(PageDraft::new("font estimate", 100.0, 100.0), At::End)
            .unwrap();
        let manual_content = edit.add_text_content(page, At::End).unwrap();
        edit.set(
            manual_content,
            &SourceText {
                text: Authored::user("manual source".to_owned()),
                language: None,
            },
        )
        .unwrap();
        let manual_layer = edit
            .add_text_layer(
                page,
                At::End,
                manual_content,
                &TextLayout {
                    origin: Origin::User,
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: None,
                },
            )
            .unwrap();
        let manual_typography = Typography {
            origin: Origin::User,
            preferred_font: Some("Manual Font".to_owned()),
            font_weight: Some(700),
            font_style: None,
            size: Some(17.0),
            auto_fit: false,
            auto_fit_size: None,
            color: None,
            stroke_color: None,
            stroke_width: None,
            alignment: None,
            writing_mode: None,
            line_height: None,
            letter_spacing: None,
            extensions: Default::default(),
        };
        edit.set(manual_layer, &manual_typography).unwrap();
        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;

        let generation = generation(PRODUCER, "test-provider").unwrap();
        let mut generated_edit = snapshot.edit_as(generation.clone());
        let generated_content = generated_edit.add_text_content(page, At::End).unwrap();
        generated_edit
            .set(
                generated_content,
                &SourceText {
                    text: Authored::generated("original".to_owned(), generation.clone()),
                    language: None,
                },
            )
            .unwrap();
        let generated_layer = generated_edit
            .add_text_layer(
                page,
                At::End,
                generated_content,
                &TextLayout {
                    origin: Origin::Generated(generation.clone()),
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: None,
                },
            )
            .unwrap();
        generated_edit
            .set(
                generated_layer,
                &Typography {
                    origin: Origin::Generated(generation),
                    preferred_font: None,
                    font_weight: None,
                    font_style: None,
                    size: None,
                    auto_fit: true,
                    auto_fit_size: None,
                    color: None,
                    stroke_color: None,
                    stroke_width: None,
                    alignment: None,
                    writing_mode: Some(WritingMode::Horizontal),
                    line_height: None,
                    letter_spacing: None,
                    extensions: Default::default(),
                },
            )
            .unwrap();
        let snapshot = session
            .commit(generated_edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;

        let mut estimates = BTreeMap::new();
        estimates.insert(generated_layer, 32.5);
        estimates.insert(manual_layer, 41.0);
        let patch = build_translation_patch(
            &snapshot,
            vec![
                (generated_content, generated_layer, "original".to_owned()),
                (manual_content, manual_layer, "manual source".to_owned()),
            ],
            vec![
                "A longer translation that should not change the estimate".to_owned(),
                "short".to_owned(),
            ],
            &estimates,
            "test-provider",
            norrix_translator::Language::Spanish,
        )
        .unwrap();
        let translated = session.commit(patch).await.unwrap().snapshot;
        assert_eq!(
            translated
                .component::<OriginalFontEstimate>(generated_layer)
                .unwrap()
                .unwrap()
                .size,
            32.5
        );
        assert_eq!(
            translated
                .component::<Typography>(generated_layer)
                .unwrap()
                .unwrap()
                .size,
            None
        );
        assert_eq!(
            translated
                .component::<Typography>(manual_layer)
                .unwrap()
                .unwrap(),
            manual_typography
        );
        assert!(
            translated
                .component::<OriginalFontEstimate>(manual_layer)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn translation_eligibility_preserves_manual_and_rejects_orphans() {
        let mut session = Session::memory().await.unwrap();
        let snapshot = session.snapshot();
        let mut edit = snapshot.edit();
        let page = edit
            .add_page(PageDraft::new("eligibility", 100.0, 100.0), At::End)
            .unwrap();

        let manual_content = edit.add_text_content(page, At::End).unwrap();
        let manual_layer = edit
            .add_text_layer(
                page,
                At::End,
                manual_content,
                &TextLayout {
                    origin: Origin::User,
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: None,
                },
            )
            .unwrap();

        let snapshot = session
            .commit(edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;
        let generated = norrix_scene::Generation::new(
            norrix_scene::ProducerId::new("dev.koharu.pipeline.test").unwrap(),
        );
        let mut generated_edit = snapshot.edit_as(generated.clone());
        let orphan_content = generated_edit.add_text_content(page, At::End).unwrap();
        let orphan_layer = generated_edit
            .add_text_layer(
                page,
                At::End,
                orphan_content,
                &TextLayout {
                    origin: Origin::Generated(generated.clone()),
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: None,
                },
            )
            .unwrap();
        let snapshot = session
            .commit(generated_edit.finish().unwrap())
            .await
            .unwrap()
            .snapshot;
        let input = StageInput::new(
            snapshot,
            page,
            None,
            None,
            std::sync::Arc::new(crate::ImageCache::default()),
            None,
        );

        assert!(valid_translation_box(&input, manual_layer, manual_content).unwrap());
        assert!(!valid_translation_box(&input, orphan_layer, orphan_content).unwrap());
    }

    #[tokio::test]
    async fn generated_translation_patch_rejects_empty_or_incomplete_results() {
        let snapshot = Session::memory().await.unwrap().snapshot();
        let id = norrix_scene::EntityId::new();
        let segment = (id, id, "source".to_owned());

        assert!(
            build_translation_patch(
                &snapshot,
                vec![segment.clone()],
                vec!["  ".to_owned()],
                &BTreeMap::new(),
                "test-provider",
                norrix_translator::Language::Spanish,
            )
            .is_err()
        );
        assert!(
            build_translation_patch(
                &snapshot,
                vec![segment],
                Vec::new(),
                &BTreeMap::new(),
                "test-provider",
                norrix_translator::Language::Spanish,
            )
            .is_err()
        );
    }
}
