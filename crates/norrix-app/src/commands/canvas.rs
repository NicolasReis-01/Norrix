use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};
use image::{GrayImage, ImageEncoder as _, codecs::png::PngEncoder};
use norrix_desktop::{
    CanvasState, Desktop, Frame, TextFontSize, TextLayoutPreview as DesktopTextLayoutPreview,
    TransformFrame,
};
use norrix_rasterizer::ResourceId;
use norrix_scene::{EntityId, Revision};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{
    AppHandle, Manager as _, State,
    ipc::{Channel, IpcResponse},
};
use tauri_runtime_cef::CefRuntime;

use super::{
    ChannelExt as _, Error, processing,
    processing::{JobChannel, JobId, Processing},
    project::{CurrentProject, Page, Project, RasterStrokeMode},
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, Type)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Type)]
pub struct PaintBrush {
    pub diameter: f32,
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
pub struct LayerCommit {
    pub revision: Revision,
    pub layer: EntityId,
}

#[derive(Type)]
#[specta(transparent)]
pub(crate) struct CanvasBytes(#[specta(type = Vec<u8>)] Vec<u8>);

#[derive(Clone, Copy, Deserialize, Type)]
#[specta(transparent)]
pub(crate) struct CanvasGeneration(#[specta(type = f64)] u64);

#[derive(Clone, Debug, Serialize, Type)]
pub struct CanvasPagePreparation {
    pub revision: Revision,
    pub page: Page,
    pub element_frames: Vec<TransformFrame>,
    pub text_font_sizes: Vec<TextFontSize>,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct TextLayoutPreview {
    pub request_revision: u32,
    pub font_size: f32,
    pub width: f32,
    pub height: f32,
    pub line_height: f32,
    pub line_breaks: Vec<u32>,
    pub path_data: String,
    pub caret_stops: Vec<TextCaretStop>,
    pub synthetic_bold_width: f32,
    pub color: [u8; 4],
    pub stroke_color: [u8; 4],
    pub stroke_width: f32,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct TextCaretStop {
    pub offset: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub line: u32,
}

impl IpcResponse for CanvasBytes {
    fn body(self) -> tauri::Result<tauri::ipc::InvokeResponseBody> {
        Ok(self.0.into())
    }
}

#[derive(Default)]
pub(crate) struct CanvasChannel {
    pub(crate) channel: Mutex<Option<Channel<CanvasState>>>,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_manifest(
    generation: CanvasGeneration,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    Ok(CanvasBytes(desktop.frame_manifest_bytes(generation.0)?))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_resource(
    generation: CanvasGeneration,
    resource: String,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    let resource = resource
        .parse::<ResourceId>()
        .context("canvas resource id is invalid")?;
    Ok(CanvasBytes(
        desktop.frame_resource_bytes(generation.0, resource)?,
    ))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn prepare_canvas_page(
    page: EntityId,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
) -> Result<Option<CanvasPagePreparation>, Error> {
    let (snapshot, prepared_page) = {
        let project = project.project.lock().await;
        let project = project.as_ref().context("no project is open")?;
        let snapshot = project.snapshot();
        let prepared_page = Project::page(&snapshot, page)?;
        (snapshot, prepared_page)
    };
    let revision = snapshot.revision();
    if !desktop.prepare_page(&snapshot, page).await? {
        return Ok(None);
    }
    let element_frames = desktop.page_element_frames(page, revision)?;
    let text_font_sizes = desktop.page_text_font_sizes(page, revision)?;
    Ok(Some(CanvasPagePreparation {
        revision,
        page: prepared_page,
        element_frames,
        text_font_sizes,
    }))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn preview_text_layout(
    page: EntityId,
    element: EntityId,
    text: String,
    request_revision: u32,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
) -> Result<TextLayoutPreview, Error> {
    let snapshot =
        {
            let project = project.project.lock().await;
            let project = project.as_ref().context("no project is open")?;
            let snapshot = project.snapshot();
            let page_view = Project::page(&snapshot, page)?;
            if !page_view.layers.iter().any(
                |layer| matches!(layer, super::project::Layer::Text { id, .. } if *id == element),
            ) {
                return Err(anyhow!(
                    "text preview target {element} is not a text layer on page {page}"
                )
                .into());
            }
            snapshot
        };
    let DesktopTextLayoutPreview {
        font_size,
        width,
        height,
        line_height,
        line_breaks,
        path_data,
        caret_stops,
        synthetic_bold_width,
        color,
        stroke_color,
        stroke_width,
    } = desktop
        .preview_text_layout(&snapshot, page, element, text)
        .await?;
    Ok(TextLayoutPreview {
        request_revision,
        font_size,
        width,
        height,
        line_height,
        line_breaks,
        path_data,
        synthetic_bold_width,
        color,
        stroke_color,
        stroke_width,
        caret_stops: caret_stops
            .into_iter()
            .map(|stop| TextCaretStop {
                offset: stop.offset,
                x: stop.x,
                y: stop.y,
                width: stop.width,
                height: stop.height,
                line: stop.line,
            })
            .collect(),
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_page_manifest(
    page: EntityId,
    revision: Revision,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    Ok(CanvasBytes(desktop.page_manifest_bytes(page, revision)?))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_page_resource(
    page: EntityId,
    revision: Revision,
    resource: String,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    let resource = resource
        .parse::<ResourceId>()
        .context("canvas resource id is invalid")?;
    Ok(CanvasBytes(
        desktop.page_resource_bytes(page, revision, resource)?,
    ))
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "point_text_added",
    skip_all,
    fields(origin = "user", point_count = 1_u64)
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn add_point_text(
    page: EntityId,
    point: Point,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    let (commit, page, layer) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        Project::page(&project.snapshot(), page)?;
        let (commit, layer) = project.add_point_text(page, point).await?;
        project.record_commit(&commit);
        (commit, project.active_page(), layer)
    };
    desktop.synchronize(&commit.snapshot, page, &commit).await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(LayerCommit {
        revision: commit.revision,
        layer,
    })
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "text_box_added",
    skip_all,
    fields(origin = "user", entity_count = 1_u64)
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn add_text_box(
    page: EntityId,
    frame: Frame,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    let (commit, page, layer) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        Project::page(&project.snapshot(), page)?;
        let (commit, layer) = project.add_text_box(page, frame).await?;
        project.record_commit(&commit);
        (commit, project.active_page(), layer)
    };
    desktop.synchronize(&commit.snapshot, page, &commit).await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(LayerCommit {
        revision: commit.revision,
        layer,
    })
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "paint_committed",
    skip_all,
    fields(origin = "user", point_count = points.len(), size = f64::from(brush.diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_paint(
    page: EntityId,
    expected_revision: Revision,
    layer: Option<EntityId>,
    points: Vec<Point>,
    brush: PaintBrush,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    commit_raster_stroke(
        page,
        expected_revision,
        layer,
        points,
        brush.diameter,
        brush.color,
        RasterStrokeMode::Paint,
        &desktop,
        &project,
        &canvas_channel,
    )
    .await
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "erase_committed",
    skip_all,
    fields(origin = "user", point_count = points.len(), size = f64::from(diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_erase(
    page: EntityId,
    expected_revision: Revision,
    layer: EntityId,
    points: Vec<Point>,
    diameter: f32,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    commit_raster_stroke(
        page,
        expected_revision,
        Some(layer),
        points,
        diameter,
        [0; 4],
        RasterStrokeMode::Erase,
        &desktop,
        &project,
        &canvas_channel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn commit_raster_stroke(
    page: EntityId,
    expected_revision: Revision,
    layer: Option<EntityId>,
    points: Vec<Point>,
    diameter: f32,
    color: [u8; 4],
    mode: RasterStrokeMode,
    desktop: &Desktop,
    project: &CurrentProject,
    canvas_channel: &CanvasChannel,
) -> Result<LayerCommit, Error> {
    let (commit, page, element) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        ensure_revision(project.snapshot().revision(), expected_revision)?;
        Project::page(&project.snapshot(), page)?;
        let (commit, element) = project
            .apply_raster_stroke(
                page,
                layer,
                mode,
                color,
                diameter,
                points
                    .into_iter()
                    .map(|point| norrix_scene::Point {
                        x: point.x,
                        y: point.y,
                    })
                    .collect(),
            )
            .await?;
        project.record_commit(&commit);
        (commit, page, element)
    };
    desktop
        .synchronize(&commit.snapshot, Some(page), &commit)
        .await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(LayerCommit {
        revision: commit.revision,
        layer: element,
    })
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "transform_committed",
    skip_all,
    fields(origin = "user", entity_count = elements.len()),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_transform(
    page: EntityId,
    expected_revision: Revision,
    elements: Vec<TransformFrame>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<Option<Revision>, Error> {
    let geometries = desktop.transform_geometries(page, expected_revision, &elements)?;
    if geometries.is_empty() {
        return Ok(None);
    }
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        ensure_revision(project.snapshot().revision(), expected_revision)?;
        let commit = project.set_geometries(geometries).await?;
        project.record_commit(&commit);
        (commit, page)
    };
    desktop
        .synchronize(&commit.snapshot, Some(page), &commit)
        .await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(Some(commit.revision))
}

#[tracing::instrument(
    target = "norrix_metrics",
    name = "inpaint_requested",
    skip_all,
    fields(origin = "user", point_count = points.len(), size = f64::from(diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_inpaint(
    page: EntityId,
    expected_revision: Revision,
    points: Vec<Point>,
    diameter: f32,
    handle: AppHandle<CefRuntime>,
    project: State<'_, CurrentProject>,
) -> Result<Option<JobId>, Error> {
    if !diameter.is_finite() || diameter <= 0.0 || points.is_empty() {
        return Err(anyhow!(
            "an inpaint stroke requires a positive diameter and at least one point"
        )
        .into());
    }
    if points
        .iter()
        .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return Err(anyhow!("inpaint stroke points must be finite").into());
    }
    let (page, width, height) = {
        let project = project.project.lock().await;
        let project = project.as_ref().context("no project is open")?;
        let snapshot = project.snapshot();
        ensure_revision(snapshot.revision(), expected_revision)?;
        Project::page(&snapshot, page)?;
        let page_value = snapshot.page(page)?.page()?;
        (
            page,
            page_value.width.round() as u32,
            page_value.height.round() as u32,
        )
    };
    let (png, bounds) =
        tokio_rayon::spawn(move || encode_mask(width, height, &points, diameter)).await?;
    *handle.state::<Processing>().inpainting_mask.lock() = Some(norrix_pipeline::InpaintingMask {
        page,
        png: Arc::from(png),
    });
    Ok(Some(
        processing::process(
            handle.clone(),
            norrix_pipeline::Scope::Region { page, bounds },
            norrix_pipeline::Operation::Only {
                stage: norrix_pipeline::Stage::Inpainting,
            },
            handle.state::<CurrentProject>(),
            handle.state::<Processing>(),
            handle.state::<JobChannel>(),
        )
        .await?,
    ))
}

fn ensure_revision(actual: Revision, expected: Revision) -> anyhow::Result<()> {
    if actual != expected {
        bail!("canvas edit expected revision {expected}, but the project is at {actual}");
    }
    Ok(())
}

fn encode_mask(
    width: u32,
    height: u32,
    points: &[Point],
    diameter: f32,
) -> anyhow::Result<(Vec<u8>, norrix_pipeline::Bounds)> {
    if width == 0 || height == 0 {
        bail!("page dimensions must be positive");
    }
    let mut image = GrayImage::new(width, height);
    let radius = f64::from(diameter) * 0.5;
    let mut dirty = None::<[u32; 4]>;
    for (start, end) in points
        .iter()
        .zip(points.iter().skip(1))
        .chain(points.last().map(|point| (point, point)))
    {
        let left = (start.x.min(end.x) - radius - 0.5).floor().max(0.0) as u32;
        let top = (start.y.min(end.y) - radius - 0.5).floor().max(0.0) as u32;
        let right = (start.x.max(end.x) + radius + 0.5)
            .ceil()
            .min(f64::from(width)) as u32;
        let bottom = (start.y.max(end.y) + radius + 0.5)
            .ceil()
            .min(f64::from(height)) as u32;
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length_squared = dx.mul_add(dx, dy * dy);
        for y in top..bottom {
            for x in left..right {
                let px = f64::from(x) + 0.5;
                let py = f64::from(y) + 0.5;
                let progress = if length_squared <= f64::EPSILON {
                    0.0
                } else {
                    (((px - start.x) * dx + (py - start.y) * dy) / length_squared).clamp(0.0, 1.0)
                };
                let nearest_x = start.x + progress * dx;
                let nearest_y = start.y + progress * dy;
                let distance = (px - nearest_x).hypot(py - nearest_y);
                let coverage = ((radius + 0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
                if coverage == 0 {
                    continue;
                }
                let pixel = image.get_pixel_mut(x, y);
                pixel.0[0] = pixel.0[0].max(coverage);
                dirty = Some(match dirty {
                    Some([min_x, min_y, max_x, max_y]) => {
                        [min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]
                    }
                    None => [x, y, x, y],
                });
            }
        }
    }
    let [left, top, right, bottom] = dirty.context("inpaint stroke does not intersect the page")?;
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(
        image.as_raw(),
        width,
        height,
        image::ExtendedColorType::L8,
    )?;
    Ok((
        png,
        norrix_pipeline::Bounds {
            x: f64::from(left),
            y: f64::from(top),
            width: f64::from(right - left + 1),
            height: f64::from(bottom - top + 1),
        },
    ))
}
