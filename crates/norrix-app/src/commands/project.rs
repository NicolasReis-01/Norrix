use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, bail};
use image::{DynamicImage, ImageFormat, RgbaImage};
use norrix_desktop::Frame;
use norrix_scene::{
    AssetInput, AssetMetadata, AssetRole, At, Authored, Commit, Edit as SceneEdit, EntityId,
    EntityOrigin, Geometry as SceneGeometry, Group as SceneGroup, Origin, PageDraft,
    PipelineProgress, Point as ScenePoint, Presents, ProgressKind, ProgressOrigin, ProgressStatus,
    RasterLayer as SceneRasterLayer, RasterLayerKind, Region as SceneRegion, RemovePolicy,
    Revision, Session, Snapshot, SourceText as SceneSourceText, TextGroup as SceneTextGroup,
    TextLayout as SceneTextLayout, TextLayoutKind, Translation as SceneTranslation,
    Typography as SceneTypography, Visibility as SceneVisibility,
};
use serde::Serialize;
use specta::Type;
use tokio::sync::Mutex;

use super::{
    canvas::Point,
    editing::{GeometryUpdate, TextContentUpdate, TextFieldUpdate, TypographyUpdate},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RasterStrokeMode {
    Paint,
    Erase,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct ProjectInfo {
    pub name: String,
    pub revision: Revision,
    pub active_page: Option<EntityId>,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct ProjectSummary {
    pub name: String,
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct PageSummary {
    pub id: EntityId,
    pub label: String,
    pub size: PageSize,
    pub source_asset: Option<String>,
    #[specta(type = f64)]
    pub layer_count: usize,
    pub detection_complete: bool,
    pub ocr_complete: bool,
    pub translation_complete: bool,
    pub inpainting_complete: bool,
    #[specta(type = f64)]
    pub translated_texts: usize,
    #[specta(type = f64)]
    pub total_texts: usize,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Page {
    pub id: EntityId,
    pub label: String,
    pub size: PageSize,
    pub layers: Vec<Layer>,
    pub regions: Vec<AnalysisRegion>,
}

#[derive(Clone, Debug, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Layer {
    Group {
        id: EntityId,
        parent: Option<EntityId>,
        visibility: LayerVisibility,
        name: String,
        role: Option<GroupRole>,
    },
    Text {
        id: EntityId,
        parent: Option<EntityId>,
        geometry: Option<Geometry>,
        angle_degrees: Option<f32>,
        visibility: LayerVisibility,
        content: Box<TextContent>,
        typography: Option<Typography>,
        layout: TextLayoutKind,
        automatic_region: Option<EntityId>,
    },
    Raster {
        id: EntityId,
        parent: Option<EntityId>,
        visibility: LayerVisibility,
        image: Option<String>,
        name: String,
        kind: RasterLayerKind,
    },
    Image {
        id: EntityId,
        parent: Option<EntityId>,
        geometry: Geometry,
        visibility: LayerVisibility,
        image: String,
    },
    Artwork {
        id: EntityId,
        parent: Option<EntityId>,
        geometry: Geometry,
        visibility: LayerVisibility,
        image: String,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GroupRole {
    Text,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct TextContent {
    pub id: EntityId,
    pub source: Option<SourceText>,
    pub translation: Option<Translation>,
    pub translation_complete: bool,
    pub role: Option<String>,
    pub source_region: Option<EntityId>,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct AnalysisRegion {
    pub id: EntityId,
    pub parent: Option<EntityId>,
    pub geometry: Geometry,
    pub kind: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Geometry {
    pub points: Vec<Point>,
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
pub struct LayerVisibility {
    pub visible: bool,
    pub opacity: f32,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct SourceText {
    pub text: String,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Translation {
    pub text: String,
    pub language: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, Serialize, Type)]
pub struct Typography {
    pub preferred_font: Option<String>,
    pub font_weight: Option<u16>,
    pub font_style: Option<norrix_scene::FontStyle>,
    pub size: Option<f32>,
    pub auto_fit: bool,
    pub color: Option<[u8; 4]>,
    pub stroke_color: Option<[u8; 4]>,
    pub stroke_width: Option<f32>,
    pub alignment: Option<norrix_scene::TextAlignment>,
    pub writing_mode: Option<norrix_scene::WritingMode>,
    pub line_height: Option<f32>,
    pub letter_spacing: Option<f32>,
}

pub(crate) struct CurrentProject {
    pub(crate) project: Mutex<Option<Project>>,
}

#[derive(Clone)]
pub(crate) struct ProjectLibrary {
    root: PathBuf,
    legacy_root: PathBuf,
}

impl ProjectLibrary {
    pub(crate) fn new() -> Result<Self> {
        let documents = dirs::document_dir().context("the Documents directory is unavailable")?;
        let root = documents.join("Norrix");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("failed to create {}", root.display()))?;
        Ok(Self {
            root,
            legacy_root: documents.join("Koharu"),
        })
    }

    pub(crate) fn list(&self) -> Result<Vec<ProjectSummary>> {
        let mut projects = HashMap::new();
        for root in [&self.legacy_root, &self.root] {
            for (last_used, project) in list_projects(root)? {
                projects.insert(project.name.clone(), (last_used, project));
            }
        }

        let mut projects = projects.into_values().collect::<Vec<_>>();
        projects.sort_unstable_by(|(left_used, left), (right_used, right)| {
            right_used
                .cmp(left_used)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(projects.into_iter().map(|(_, project)| project).collect())
    }

    pub(crate) async fn create(&self, name: &str) -> Result<Project> {
        let (name, path) = self.resolve(name)?;
        let legacy_path = self.legacy_root.join(format!("{name}.khrproj"));
        if path
            .try_exists()
            .with_context(|| format!("failed to inspect {}", path.display()))?
            || legacy_path
                .try_exists()
                .with_context(|| format!("failed to inspect {}", legacy_path.display()))?
        {
            anyhow::bail!("project {name:?} already exists");
        }
        Project::create(name, path).await
    }

    pub(crate) async fn open(&self, name: &str) -> Result<Project> {
        let (name, path) = self.resolve_for_open(name)?;
        Project::open(name, path).await
    }

    pub(crate) fn delete(&self, name: &str) -> Result<()> {
        let (name, current) = self.resolve(name)?;
        let legacy = self.legacy_root.join(format!("{name}.khrproj"));
        let mut removed = false;

        for path in [current, legacy] {
            if !path
                .try_exists()
                .with_context(|| format!("failed to inspect {}", path.display()))?
            {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("failed to inspect {}", path.display()))?;
            if metadata.file_type().is_dir() {
                std::fs::remove_dir_all(&path)
                    .with_context(|| format!("failed to delete {}", path.display()))?;
            } else {
                std::fs::remove_file(&path)
                    .with_context(|| format!("failed to delete {}", path.display()))?;
            }
            removed = true;
        }

        if !removed {
            bail!("project {name:?} does not exist");
        }
        Ok(())
    }

    fn resolve(&self, name: &str) -> Result<(String, PathBuf)> {
        let name = validate_project_name(name)?;
        Ok((name.clone(), self.root.join(format!("{name}.khrproj"))))
    }

    fn resolve_for_open(&self, name: &str) -> Result<(String, PathBuf)> {
        let (name, current) = self.resolve(name)?;
        if current
            .try_exists()
            .with_context(|| format!("failed to inspect {}", current.display()))?
        {
            return Ok((name, current));
        }

        let legacy = self.legacy_root.join(format!("{name}.khrproj"));
        if legacy
            .try_exists()
            .with_context(|| format!("failed to inspect {}", legacy.display()))?
        {
            copy_legacy_project(&legacy, &current, &self.root)?;
        }
        Ok((name, current))
    }
}

fn copy_legacy_project(source: &Path, destination: &Path, staging_root: &Path) -> Result<()> {
    if destination
        .try_exists()
        .with_context(|| format!("failed to inspect {}", destination.display()))?
    {
        return Ok(());
    }

    if !source.is_dir() {
        anyhow::bail!("legacy project is not a directory: {}", source.display());
    }

    let staging = tempfile::Builder::new()
        .prefix(".project-import-")
        .tempdir_in(staging_root)
        .with_context(|| {
            format!(
                "failed to stage project import in {}",
                staging_root.display()
            )
        })?;
    copy_project_contents(source, staging.path())?;

    match std::fs::rename(staging.path(), destination) {
        Ok(()) => Ok(()),
        Err(_)
            if destination
                .try_exists()
                .with_context(|| format!("failed to inspect {}", destination.display()))? =>
        {
            Ok(())
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to publish imported project at {}",
                destination.display()
            )
        }),
    }
}

fn copy_project_contents(source: &Path, destination: &Path) -> Result<()> {
    for entry in
        std::fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read {}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", source_path.display()))?;

        if file_type.is_dir() {
            std::fs::create_dir(&destination_path)
                .with_context(|| format!("failed to create {}", destination_path.display()))?;
            copy_project_contents(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy project file from {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        } else {
            anyhow::bail!(
                "cannot safely import unsupported project entry {}",
                source_path.display()
            );
        }
    }
    Ok(())
}

fn list_projects(root: &Path) -> Result<Vec<(std::time::SystemTime, ProjectSummary)>> {
    if !root
        .try_exists()
        .with_context(|| format!("failed to inspect {}", root.display()))?
    {
        return Ok(Vec::new());
    }

    let projects = std::fs::read_dir(root)
        .with_context(|| format!("failed to read {}", root.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let path = entry.path();
            let is_project_directory = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("khrproj"))
                && (path.join("state-a.khr").is_file() || path.join("state-b.khr").is_file());
            if !is_project_directory {
                return None;
            }
            let last_used = ["state-a.khr", "state-b.khr"]
                .into_iter()
                .filter_map(|file| std::fs::metadata(path.join(file)).ok()?.modified().ok())
                .max()
                .unwrap_or(std::time::UNIX_EPOCH);
            Some((
                last_used,
                ProjectSummary {
                    name: path.file_stem()?.to_str()?.to_owned(),
                },
            ))
        })
        .collect();
    Ok(projects)
}

pub(crate) struct Project {
    pub(crate) session: Session,
    pub(crate) name: String,
    pub(crate) active_page: Option<EntityId>,
    pub(crate) undo: Vec<Vec<Revision>>,
    pub(crate) redo: Vec<Vec<Revision>>,
    text_edit_session: Option<String>,
}

impl Project {
    pub(crate) async fn create(name: String, path: PathBuf) -> Result<Self> {
        let session = Session::create(&path)
            .await
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self::new(session, name))
    }

    pub(crate) async fn open(name: String, path: PathBuf) -> Result<Self> {
        let session = Session::open(&path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(Self::new(session, name))
    }

    fn new(session: Session, name: String) -> Self {
        let active_page = session.snapshot().pages().next().map(|page| page.id());
        Self {
            session,
            name,
            active_page,
            undo: Vec::new(),
            redo: Vec::new(),
            text_edit_session: None,
        }
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        self.session.snapshot()
    }

    pub(crate) fn revision(&self) -> Revision {
        self.snapshot().revision()
    }

    pub(crate) fn active_page(&self) -> Option<EntityId> {
        self.active_page
    }

    pub(crate) fn page_for_layer(&self, layer: EntityId) -> Result<EntityId> {
        let snapshot = self.snapshot();
        snapshot.text_layer(layer)?;
        let mut current = layer;
        loop {
            if snapshot.page(current).is_ok() {
                return Ok(current);
            }
            current = snapshot
                .parent(current)?
                .context("text layer is not attached to a page")?;
        }
    }

    pub(crate) fn select_page(&mut self, page: EntityId) -> Result<()> {
        self.snapshot().page(page)?;
        self.active_page = Some(page);
        Ok(())
    }

    pub(crate) fn reconcile_page(&mut self) {
        let snapshot = self.snapshot();
        if self
            .active_page
            .is_none_or(|page| snapshot.page(page).is_err())
        {
            self.active_page = snapshot.pages().next().map(|page| page.id());
        }
    }

    pub(crate) fn info(&self) -> ProjectInfo {
        ProjectInfo {
            name: self.name.clone(),
            revision: self.revision(),
            active_page: self.active_page,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
        }
    }

    pub(crate) fn pages(snapshot: &Snapshot) -> Result<Vec<PageSummary>> {
        snapshot
            .pages()
            .map(|page| {
                let value = page.page()?;
                let source_asset = Self::asset_id(snapshot, page.id(), "source")?;
                let layer_count = snapshot
                    .descendants(page.id())?
                    .map(|entity| Self::is_content_layer(snapshot, entity.id()))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|present| *present)
                    .count()
                    + usize::from(source_asset.is_some());
                let progress = snapshot
                    .component::<PipelineProgress>(page.id())?
                    .unwrap_or_default();
                let (translated_texts, total_texts) =
                    Self::translation_progress(snapshot, page.id())?;
                let translation_complete = if total_texts > 0 {
                    translated_texts == total_texts
                } else {
                    progress.is_complete(ProgressKind::Translation)
                };
                Ok(PageSummary {
                    id: page.id(),
                    label: value.label,
                    size: PageSize {
                        width: value.width,
                        height: value.height,
                    },
                    source_asset,
                    layer_count,
                    detection_complete: progress.is_complete(ProgressKind::Detection),
                    ocr_complete: progress.is_complete(ProgressKind::Ocr),
                    translation_complete,
                    inpainting_complete: progress.is_complete(ProgressKind::Inpainting),
                    translated_texts,
                    total_texts,
                })
            })
            .collect()
    }

    pub(crate) async fn set_progress_markers(
        &mut self,
        pages: Vec<EntityId>,
        text_layer: Option<EntityId>,
        kind: ProgressKind,
        completed: bool,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let patch = match kind {
            ProgressKind::Translation => match (pages.is_empty(), text_layer) {
                (true, Some(layer)) => {
                    let content = Self::text_content(&snapshot, layer)?;
                    let page = self.page_for_layer(layer)?;
                    snapshot.patch(|edit| {
                        let mut progress = snapshot
                            .component::<PipelineProgress>(content)?
                            .unwrap_or_default();
                        progress.set(
                            kind,
                            Some(if completed {
                                ProgressStatus::Complete(ProgressOrigin::Manual)
                            } else {
                                ProgressStatus::Pending(ProgressOrigin::Manual)
                            }),
                        );
                        edit.set(content, &progress)?;
                        Self::clear_page_translation_marker(&snapshot, edit, page)?;
                        if !completed {
                            Self::invalidate_fill_for_page(&snapshot, edit, page)?;
                        }
                        Ok(())
                    })?
                }
                (false, None) => {
                    let pages = pages.into_iter().collect::<BTreeSet<_>>();
                    for page in &pages {
                        snapshot.page(*page)?;
                    }
                    snapshot.patch(|edit| {
                        for page in pages {
                            let mut contents = Vec::new();
                            if let Some(group) = snapshot.page(page)?.text_group()? {
                                for layer in group.text_layers()? {
                                    contents.push(layer.content()?.id());
                                }
                            }
                            if contents.is_empty() {
                                let mut progress = snapshot
                                    .component::<PipelineProgress>(page)?
                                    .unwrap_or_default();
                                progress.set(
                                    kind,
                                    Some(if completed {
                                        ProgressStatus::Complete(ProgressOrigin::Manual)
                                    } else {
                                        ProgressStatus::Pending(ProgressOrigin::Manual)
                                    }),
                                );
                                edit.set(page, &progress)?;
                            } else {
                                for content in contents {
                                    let mut progress = snapshot
                                        .component::<PipelineProgress>(content)?
                                        .unwrap_or_default();
                                    progress.set(
                                        kind,
                                        Some(if completed {
                                            ProgressStatus::Complete(ProgressOrigin::Manual)
                                        } else {
                                            ProgressStatus::Pending(ProgressOrigin::Manual)
                                        }),
                                    );
                                    edit.set(content, &progress)?;
                                }
                                Self::clear_page_translation_marker(&snapshot, edit, page)?;
                            }
                        }
                        Ok(())
                    })?
                }
                _ => bail!("select either one or more pages or one text box"),
            },
            ProgressKind::Detection | ProgressKind::Ocr | ProgressKind::Inpainting => {
                if text_layer.is_some() || pages.is_empty() {
                    bail!("select one or more pages to change page progress");
                }

                let pages = pages.into_iter().collect::<BTreeSet<_>>();
                for page in &pages {
                    snapshot.page(*page)?;
                }
                snapshot.patch(|edit| {
                    for page in pages {
                        let mut progress = snapshot
                            .component::<PipelineProgress>(page)?
                            .unwrap_or_default();
                        progress.set(
                            kind,
                            completed.then_some(ProgressStatus::Complete(ProgressOrigin::Manual)),
                        );
                        if !completed {
                            match kind {
                                ProgressKind::Detection => {
                                    progress.ocr = None;
                                    progress.inpainting = None;
                                    Self::invalidate_translations_for_page(&snapshot, edit, page)?;
                                }
                                ProgressKind::Ocr => {
                                    progress.inpainting = None;
                                    Self::invalidate_translations_for_page(&snapshot, edit, page)?;
                                }
                                ProgressKind::Inpainting => {}
                                ProgressKind::Translation => unreachable!(),
                            }
                        }
                        edit.set(page, &progress)?;
                    }
                    Ok(())
                })?
            }
        };
        self.commit(patch).await
    }

    fn clear_page_translation_marker(
        snapshot: &Snapshot,
        edit: &mut SceneEdit,
        page: EntityId,
    ) -> norrix_scene::Result<()> {
        let mut progress = snapshot
            .component::<PipelineProgress>(page)?
            .unwrap_or_default();
        if progress.status(ProgressKind::Translation).is_some() {
            progress.set(ProgressKind::Translation, None);
            edit.set(page, &progress)?;
        }
        Ok(())
    }

    fn translation_progress(snapshot: &Snapshot, page: EntityId) -> Result<(usize, usize)> {
        let Some(group) = snapshot.page(page)?.text_group()? else {
            return Ok((0, 0));
        };
        let mut completed = 0;
        let mut total = 0;
        for layer in group.text_layers()? {
            let content = layer.content()?;
            let progress = snapshot
                .component::<PipelineProgress>(content.id())?
                .and_then(|progress| progress.status(ProgressKind::Translation));
            let complete = match progress {
                Some(ProgressStatus::Complete(_)) => true,
                Some(ProgressStatus::Pending(_)) => false,
                None => snapshot
                    .component::<SceneTranslation>(content.id())?
                    .is_some_and(|translation| !translation.text.value.trim().is_empty()),
            };
            completed += usize::from(complete);
            total += 1;
        }
        Ok((completed, total))
    }

    fn invalidate_translation_for_content(
        snapshot: &Snapshot,
        edit: &mut SceneEdit,
        content: EntityId,
    ) -> norrix_scene::Result<()> {
        let mut progress = snapshot
            .component::<PipelineProgress>(content)?
            .unwrap_or_default();
        let marked_complete = progress
            .status(ProgressKind::Translation)
            .is_some_and(|status| matches!(status, ProgressStatus::Complete(_)));
        let has_translation = snapshot
            .component::<SceneTranslation>(content)?
            .is_some_and(|translation| !translation.text.value.trim().is_empty());
        if marked_complete || has_translation {
            progress.set(
                ProgressKind::Translation,
                Some(ProgressStatus::Pending(ProgressOrigin::Automatic)),
            );
            edit.set(content, &progress)?;
        }
        Ok(())
    }

    fn invalidate_translations_for_page(
        snapshot: &Snapshot,
        edit: &mut SceneEdit,
        page: EntityId,
    ) -> norrix_scene::Result<()> {
        let Some(group) = snapshot.page(page)?.text_group()? else {
            return Ok(());
        };
        for layer in group.text_layers()? {
            Self::invalidate_translation_for_content(snapshot, edit, layer.content()?.id())?;
        }
        Ok(())
    }

    fn invalidate_fill_for_page(
        snapshot: &Snapshot,
        edit: &mut SceneEdit,
        page: EntityId,
    ) -> norrix_scene::Result<()> {
        let mut progress = snapshot
            .component::<PipelineProgress>(page)?
            .unwrap_or_default();
        if progress.inpainting.take().is_some() {
            edit.set(page, &progress)?;
        }
        Ok(())
    }

    pub(crate) async fn rename_page(&mut self, page: EntityId, label: String) -> Result<Commit> {
        let snapshot = self.snapshot();
        let current = snapshot.page(page)?.page()?;
        let patch = snapshot.patch(|edit| {
            edit.set_page(page, PageDraft::new(label, current.width, current.height))
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn delete_pages(&mut self, pages: Vec<EntityId>) -> Result<Commit> {
        let snapshot = self.snapshot();
        let pages = Self::unique_roots(&snapshot, pages)?;
        let patch = snapshot.patch(|edit| {
            for page in pages {
                edit.remove_entity(page, RemovePolicy::Cascade)?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn move_page(&mut self, page: EntityId, index: usize) -> Result<Commit> {
        let snapshot = self.snapshot();
        let siblings = snapshot.pages().map(|page| page.id()).collect::<Vec<_>>();
        let at = Self::placement(&siblings, page, index);
        let patch = snapshot.patch(|edit| edit.move_entity(page, None, at))?;
        self.commit(patch).await
    }

    pub(crate) async fn add_point_text(
        &mut self,
        page: EntityId,
        point: Point,
    ) -> Result<(Commit, EntityId)> {
        if !point.x.is_finite() || !point.y.is_finite() {
            bail!("text position must contain finite coordinates");
        }
        let value = self.snapshot().page(page)?.page()?;
        if point.x < 0.0 || point.y < 0.0 || point.x >= value.width || point.y >= value.height {
            bail!("text position must be inside the page");
        }
        self.add_text(
            page,
            Frame {
                x: point.x as f32,
                y: point.y as f32,
                width: (value.width - point.x).clamp(1.0, 320.0) as f32,
                height: (value.height - point.y).clamp(1.0, 120.0) as f32,
                angle_degrees: 0.0,
            },
            TextLayoutKind::Point,
        )
        .await
    }

    pub(crate) async fn add_text_box(
        &mut self,
        page: EntityId,
        frame: Frame,
    ) -> Result<(Commit, EntityId)> {
        self.add_text(page, frame, TextLayoutKind::Paragraph).await
    }

    async fn add_text(
        &mut self,
        page: EntityId,
        frame: Frame,
        kind: TextLayoutKind,
    ) -> Result<(Commit, EntityId)> {
        let snapshot = self.snapshot();
        let geometry = Self::geometry_from_frame(frame)?;
        let mut layer = None;
        let patch = snapshot.patch(|edit| {
            let content = edit.add_text_content(page, At::End)?;
            edit.set(
                content,
                &SceneSourceText {
                    text: Authored::user(String::new()),
                    language: None,
                },
            )?;
            edit.set(
                content,
                &SceneTranslation {
                    text: Authored::user(String::new()),
                    language: None,
                },
            )?;
            let added_layer = edit.add_text_layer(
                page,
                At::End,
                content,
                &SceneTextLayout {
                    origin: Origin::User,
                    kind,
                    angle_degrees: Some(frame.angle_degrees),
                },
            )?;
            layer = Some(added_layer);
            edit.set(added_layer, &geometry)?;
            edit.set(
                added_layer,
                &SceneTypography {
                    origin: Origin::User,
                    preferred_font: None,
                    font_weight: None,
                    font_style: None,
                    size: None,
                    auto_fit: true,
                    auto_fit_size: None,
                    color: None,
                    stroke_color: None,
                    stroke_width: None,
                    alignment: match kind {
                        TextLayoutKind::Point => Some(norrix_scene::TextAlignment::Start),
                        TextLayoutKind::Paragraph => None,
                    },
                    writing_mode: None,
                    line_height: None,
                    letter_spacing: None,
                    extensions: Default::default(),
                },
            )?;
            Ok(())
        })?;
        Ok((
            self.commit(patch).await?,
            layer.expect("text layer was added while building the patch"),
        ))
    }

    pub(crate) async fn set_source_text(
        &mut self,
        layer: EntityId,
        text: String,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let content = Self::text_content(&snapshot, layer)?;
        let previous = snapshot.component::<SceneSourceText>(content)?;
        let language = previous.as_ref().and_then(|source| source.language.clone());
        let source_changed = previous.is_none_or(|source| source.text.value != text);
        let page = self.page_for_layer(layer)?;
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            edit.promote_entity_to_user(content)?;
            edit.set(
                content,
                &SceneSourceText {
                    text: Authored::user(text.clone()),
                    language,
                },
            )?;
            if source_changed {
                Self::invalidate_translation_for_content(&snapshot, edit, content)?;
                Self::invalidate_fill_for_page(&snapshot, edit, page)?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_translation(
        &mut self,
        layer: EntityId,
        text: Option<String>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let content = Self::text_content(&snapshot, layer)?;
        let previous = snapshot.component::<SceneTranslation>(content)?;
        let language = previous
            .as_ref()
            .and_then(|translation| translation.language.clone());
        let translation_changed = match (&previous, &text) {
            (Some(previous), Some(next)) => previous.text.value != *next,
            (Some(_), None) => true,
            (None, Some(_)) => true,
            (None, None) => false,
        };
        let translation_complete = text
            .as_deref()
            .is_some_and(|translation| !translation.trim().is_empty());
        let page = self.page_for_layer(layer)?;
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            edit.promote_entity_to_user(content)?;
            match text {
                Some(text) => {
                    edit.set(
                        content,
                        &SceneTranslation {
                            text: Authored::user(text),
                            language,
                        },
                    )?;
                    let mut progress = snapshot
                        .component::<PipelineProgress>(content)?
                        .unwrap_or_default();
                    progress.set(
                        ProgressKind::Translation,
                        Some(if translation_complete {
                            ProgressStatus::Complete(ProgressOrigin::Manual)
                        } else {
                            ProgressStatus::Pending(ProgressOrigin::Manual)
                        }),
                    );
                    edit.set(content, &progress)?;
                }
                None => {
                    edit.remove::<SceneTranslation>(content)?;
                    let mut progress = snapshot
                        .component::<PipelineProgress>(content)?
                        .unwrap_or_default();
                    progress.set(
                        ProgressKind::Translation,
                        Some(ProgressStatus::Pending(ProgressOrigin::Manual)),
                    );
                    edit.set(content, &progress)?;
                }
            };
            if translation_changed {
                Self::invalidate_fill_for_page(&snapshot, edit, page)?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_text_content(
        &mut self,
        updates: Vec<TextContentUpdate>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let updates = updates
            .into_iter()
            .map(|update| {
                let content = Self::text_content(&snapshot, update.layer)?;
                let page = self.page_for_layer(update.layer)?;
                Ok((update, content, page))
            })
            .collect::<Result<Vec<_>>>()?;
        let patch = snapshot.patch(|edit| {
            for (update, content, page) in updates {
                edit.promote_entity_to_user(update.layer)?;
                edit.promote_entity_to_user(content)?;
                match update.value {
                    TextFieldUpdate::Source(text) => {
                        let previous = snapshot.component::<SceneSourceText>(content)?;
                        let source_changed = previous
                            .as_ref()
                            .is_none_or(|source| source.text.value != text);
                        let language = previous.as_ref().and_then(|source| source.language.clone());
                        edit.set(
                            content,
                            &SceneSourceText {
                                text: Authored::user(text),
                                language,
                            },
                        )?;
                        if source_changed {
                            Self::invalidate_translation_for_content(&snapshot, edit, content)?;
                            Self::invalidate_fill_for_page(&snapshot, edit, page)?;
                        }
                    }
                    TextFieldUpdate::Translation(text) => {
                        let previous = snapshot.component::<SceneTranslation>(content)?;
                        let language = previous
                            .as_ref()
                            .and_then(|translation| translation.language.clone());
                        let translation_complete = text
                            .as_deref()
                            .is_some_and(|translation| !translation.trim().is_empty());
                        let translation_changed = match (&previous, &text) {
                            (Some(previous), Some(next)) => previous.text.value != *next,
                            (Some(_), None) => true,
                            (None, Some(_)) => true,
                            (None, None) => false,
                        };
                        match text {
                            Some(text) => {
                                edit.set(
                                    content,
                                    &SceneTranslation {
                                        text: Authored::user(text),
                                        language,
                                    },
                                )?;
                                let mut progress = snapshot
                                    .component::<PipelineProgress>(content)?
                                    .unwrap_or_default();
                                progress.set(
                                    ProgressKind::Translation,
                                    Some(if translation_complete {
                                        ProgressStatus::Complete(ProgressOrigin::Manual)
                                    } else {
                                        ProgressStatus::Pending(ProgressOrigin::Manual)
                                    }),
                                );
                                edit.set(content, &progress)?;
                            }
                            None => {
                                edit.remove::<SceneTranslation>(content)?;
                                let mut progress = snapshot
                                    .component::<PipelineProgress>(content)?
                                    .unwrap_or_default();
                                progress.set(
                                    ProgressKind::Translation,
                                    Some(ProgressStatus::Pending(ProgressOrigin::Manual)),
                                );
                                edit.set(content, &progress)?;
                            }
                        }
                        if translation_changed {
                            Self::invalidate_fill_for_page(&snapshot, edit, page)?;
                        }
                    }
                }
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_typography(
        &mut self,
        updates: Vec<TypographyUpdate>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let updates = updates
            .into_iter()
            .map(|update| {
                let content = Self::text_content(&snapshot, update.layer)?;
                Ok((update, content))
            })
            .collect::<Result<Vec<_>>>()?;
        let patch = snapshot.patch(|edit| {
            for (update, content) in updates {
                edit.promote_entity_to_user(update.layer)?;
                edit.promote_entity_to_user(content)?;
                edit.set(
                    update.layer,
                    &SceneTypography {
                        origin: Origin::User,
                        preferred_font: update.typography.preferred_font,
                        font_weight: update.typography.font_weight,
                        font_style: update.typography.font_style,
                        size: update.typography.size.filter(|size| *size > 0.0),
                        auto_fit: update.typography.auto_fit,
                        auto_fit_size: None,
                        color: update.typography.color,
                        stroke_color: update.typography.stroke_color,
                        stroke_width: update.typography.stroke_width,
                        alignment: update.typography.alignment,
                        writing_mode: update.typography.writing_mode,
                        line_height: update.typography.line_height,
                        letter_spacing: update.typography.letter_spacing,
                        extensions: Default::default(),
                    },
                )?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_geometry(&mut self, updates: Vec<GeometryUpdate>) -> Result<Commit> {
        let snapshot = self.snapshot();
        let updates = updates
            .into_iter()
            .map(|update| {
                let layout = snapshot
                    .component::<SceneTextLayout>(update.layer)?
                    .context("only text layers can change geometry")?;
                let content = Self::text_content(&snapshot, update.layer)?;
                if update.points.is_none()
                    && snapshot
                        .text_layer(update.layer)?
                        .automatic_target()?
                        .is_none()
                {
                    bail!("only automatically placed text can reset its geometry");
                }
                Ok((update, content, layout))
            })
            .collect::<Result<Vec<_>>>()?;
        let patch = snapshot.patch(|edit| {
            for (update, content, mut layout) in updates {
                edit.promote_entity_to_user(update.layer)?;
                edit.promote_entity_to_user(content)?;
                layout.origin = Origin::User;
                layout.angle_degrees = None;
                edit.set(update.layer, &layout)?;
                match update.points {
                    Some(points) => edit.set(
                        update.layer,
                        &SceneGeometry {
                            origin: Origin::User,
                            points: points
                                .into_iter()
                                .map(|point| ScenePoint {
                                    x: point.x,
                                    y: point.y,
                                })
                                .collect(),
                        },
                    )?,
                    None => edit.remove::<SceneGeometry>(update.layer)?,
                }
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_visibility(
        &mut self,
        layers: Vec<EntityId>,
        visible: Option<bool>,
        opacity: Option<f32>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let patch = snapshot.patch(|edit| {
            for layer in layers {
                let mut value =
                    snapshot
                        .component::<SceneVisibility>(layer)?
                        .unwrap_or(SceneVisibility {
                            origin: Origin::User,
                            visible: true,
                            opacity: 1.0,
                        });
                if let Some(visible) = visible {
                    value.visible = visible;
                }
                if let Some(opacity) = opacity {
                    value.opacity = opacity;
                }
                value.origin = Origin::User;
                edit.set(layer, &value)?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn delete_layers(&mut self, layers: Vec<EntityId>) -> Result<Commit> {
        let snapshot = self.snapshot();
        let mut expanded = Vec::new();
        for layer in layers {
            if snapshot.component::<SceneTextGroup>(layer)?.is_some() {
                expanded.extend(snapshot.children(layer)?);
            } else {
                expanded.push(layer);
            }
        }
        let layers = Self::unique_roots(&snapshot, expanded)?;
        let mut orphaned_contents = Vec::new();
        for layer in &layers {
            let Some(relation) = snapshot.relation_from::<Presents>(*layer)? else {
                continue;
            };
            let content = relation.value().target;
            if snapshot.relations_to_as::<Presents>(content).count() == 1 {
                orphaned_contents.push(content);
            }
        }
        let patch = snapshot.patch(|edit| {
            for layer in layers {
                if snapshot.page(layer).is_ok() {
                    return Err(norrix_scene::Error::Invalid(
                        "delete pages with delete_pages".to_owned(),
                    ));
                }
                edit.remove_entity(layer, RemovePolicy::Cascade)?;
            }
            for content in orphaned_contents {
                if snapshot.entity(content).is_ok() {
                    edit.remove_entity(content, RemovePolicy::Cascade)?;
                }
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    #[tracing::instrument(level = "info", skip_all, fields(layer = %layer, parent = %parent))]
    pub(crate) async fn move_layer(
        &mut self,
        layer: EntityId,
        parent: EntityId,
        index: usize,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let siblings = snapshot
            .children(parent)?
            .map(|candidate| Ok(Self::is_layer(&snapshot, candidate)?.then_some(candidate)))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let at = Self::placement(&siblings, layer, index);
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            if snapshot.component::<SceneTextGroup>(parent)?.is_some() {
                edit.promote_entity_to_user(parent)?;
            }
            edit.move_entity(layer, Some(parent), at)
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_geometries(
        &mut self,
        geometries: impl IntoIterator<Item = (EntityId, SceneGeometry, f32, bool, f32)>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let geometries = geometries
            .into_iter()
            .map(
                |(element, geometry, angle_degrees, diagonal_resize, rendered_font_size)| {
                    let mut layout = snapshot
                        .component::<SceneTextLayout>(element)?
                        .context("only text layers can change geometry")?;
                    layout.origin = Origin::User;
                    layout.angle_degrees = Some(angle_degrees);
                    let content = Self::text_content(&snapshot, element)?;
                    let existing_typography = snapshot.component::<SceneTypography>(element)?;
                    let typography = if existing_typography
                        .as_ref()
                        .is_none_or(|typography| typography.auto_fit)
                    {
                        let mut typography = existing_typography.unwrap_or(SceneTypography {
                            origin: Origin::User,
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
                        });
                        typography.origin = Origin::User;
                        typography.auto_fit = true;
                        typography.auto_fit_size = if diagonal_resize {
                            None
                        } else {
                            Some(rendered_font_size)
                        };
                        Some(typography)
                    } else {
                        None
                    };
                    Ok((element, geometry, content, layout, typography))
                },
            )
            .collect::<Result<Vec<_>>>()?;
        let patch = snapshot.patch(|edit| {
            for (element, mut geometry, content, layout, typography) in geometries {
                edit.promote_entity_to_user(element)?;
                edit.promote_entity_to_user(content)?;
                geometry.origin = Origin::User;
                edit.set(element, &geometry)?;
                edit.set(element, &layout)?;
                if let Some(typography) = typography {
                    edit.set(element, &typography)?;
                }
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn apply_raster_stroke(
        &mut self,
        page: EntityId,
        layer: Option<EntityId>,
        mode: RasterStrokeMode,
        color: [u8; 4],
        diameter: f32,
        points: Vec<ScenePoint>,
    ) -> Result<(Commit, EntityId)> {
        if !diameter.is_finite() || diameter <= 0.0 || points.is_empty() {
            bail!("a raster stroke requires a positive diameter and at least one point");
        }
        if points
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            bail!("raster stroke points must be finite");
        }
        let snapshot = self.snapshot();
        let page_value = snapshot.page(page)?.page()?;
        let width = page_value.width.round() as u32;
        let height = page_value.height.round() as u32;
        if width == 0 || height == 0 {
            bail!("page dimensions must be positive");
        }
        let mut raster_layer = None;
        let mut promote_layer = false;
        let mut image = if let Some(layer) = layer {
            if snapshot.parent(layer)? != Some(page) {
                bail!("the raster target must be a layer on the active page");
            }
            let target = snapshot
                .component::<SceneRasterLayer>(layer)?
                .context("the raster target must be a layer on the active page")?;
            promote_layer = target.origin != Origin::User
                || snapshot
                    .component::<EntityOrigin>(layer)?
                    .is_some_and(|origin| origin.origin != Origin::User);
            raster_layer = Some(target);
            match snapshot.asset(layer, &AssetRole::new("source")?)? {
                Some(asset) => {
                    let bytes = snapshot.read_blob(asset.blob).await?;
                    image::load_from_memory(&bytes)?.to_rgba8()
                }
                None => RgbaImage::new(width, height),
            }
        } else {
            if mode == RasterStrokeMode::Erase {
                bail!("eraser requires a raster layer target");
            }
            RgbaImage::new(width, height)
        };
        if image.dimensions() != (width, height) {
            bail!("raster layer dimensions must match the page");
        }
        rasterize_stroke(&mut image, mode, color, diameter, &points);
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut bytes, ImageFormat::Png)?;
        let source = AssetRole::new("source")?;
        let name = format!(
            "Paint {}",
            snapshot
                .children(page)?
                .filter(|entity| {
                    snapshot
                        .component::<SceneRasterLayer>(*entity)
                        .ok()
                        .flatten()
                        .is_some_and(|layer| layer.kind == RasterLayerKind::Paint)
                })
                .count()
                + 1
        );
        let mut committed_layer = layer;
        let patch = snapshot.patch(|edit| {
            let layer = if let Some(layer) = committed_layer {
                if promote_layer {
                    edit.promote_entity_to_user(layer)?;
                    let raster = raster_layer
                        .as_mut()
                        .expect("an existing raster target has a layer component");
                    raster.origin = Origin::User;
                    edit.set(layer, raster)?;
                }
                layer
            } else {
                let at = snapshot
                    .page(page)?
                    .text_group()?
                    .map_or(At::End, |group| At::Before(group.id()));
                let layer = edit.add_entity(page, at)?;
                edit.set(
                    layer,
                    &SceneRasterLayer {
                        origin: Origin::User,
                        name,
                        kind: RasterLayerKind::Paint,
                    },
                )?;
                committed_layer = Some(layer);
                layer
            };
            edit.set_asset(
                layer,
                &source,
                AssetInput::new(
                    bytes.into_inner(),
                    "image/png",
                    AssetMetadata {
                        width: Some(width),
                        height: Some(height),
                        attributes: Default::default(),
                    },
                ),
            )
        })?;
        Ok((
            self.commit(patch).await?,
            committed_layer.expect("raster layer was selected or added while building the patch"),
        ))
    }

    pub(crate) async fn undo(&mut self) -> Result<Commit> {
        self.text_edit_session = None;
        let revisions = self.undo.pop().ok_or_else(|| anyhow!("nothing to undo"))?;
        let commit = match self.session.undo_many(revisions.iter().copied()).await {
            Ok(commit) => commit,
            Err(error) => {
                self.undo.push(revisions);
                return Err(error.into());
            }
        };
        self.redo.push(vec![commit.revision]);
        Ok(commit)
    }

    pub(crate) async fn redo(&mut self) -> Result<Commit> {
        self.text_edit_session = None;
        let revisions = self.redo.pop().ok_or_else(|| anyhow!("nothing to redo"))?;
        let commit = match self.session.undo_many(revisions.iter().copied()).await {
            Ok(commit) => commit,
            Err(error) => {
                self.redo.push(revisions);
                return Err(error.into());
            }
        };
        self.undo.push(vec![commit.revision]);
        Ok(commit)
    }

    pub(crate) fn record(&mut self, revisions: Vec<Revision>) {
        self.text_edit_session = None;
        if !revisions.is_empty() {
            self.undo.push(revisions);
            self.redo.clear();
        }
    }

    pub(crate) fn record_commit(&mut self, commit: &Commit) {
        if commit.changes.to != commit.changes.from {
            self.record(vec![commit.revision]);
        }
    }

    pub(crate) fn record_text_edit_commit(&mut self, commit: &Commit, session: String) {
        if commit.changes.to == commit.changes.from {
            return;
        }

        if self.text_edit_session.as_deref() == Some(&session) {
            if let Some(revisions) = self.undo.last_mut() {
                revisions.push(commit.revision);
            } else {
                self.undo.push(vec![commit.revision]);
            }
        } else {
            self.undo.push(vec![commit.revision]);
            self.text_edit_session = Some(session);
        }
        self.redo.clear();
    }

    pub(crate) async fn commit_rebased(
        &mut self,
        patch: norrix_scene::Patch,
    ) -> Result<Option<Commit>> {
        let current = self.snapshot();
        let patch = match patch.rebase_on(&current) {
            Ok(patch) => patch,
            Err(
                error @ (norrix_scene::Error::PatchConflict(_)
                | norrix_scene::Error::EntityNotFound(_)
                | norrix_scene::Error::RelationNotFound(_)),
            ) => {
                tracing::debug!(%error, "pipeline output was superseded by a document edit");
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if patch.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.commit(patch).await?))
    }

    async fn commit(&mut self, patch: norrix_scene::Patch) -> Result<Commit> {
        Ok(self.session.commit(patch).await?)
    }

    pub(crate) fn page(snapshot: &Snapshot, page: EntityId) -> Result<Page> {
        let value = snapshot.page(page)?.page()?;
        let source = Self::asset_id(snapshot, page, "source")?;
        let mut layers = Vec::new();
        Self::collect_layer_views(snapshot, page, &mut layers)?;
        if let Some(image) = source.clone() {
            layers.insert(
                0,
                Layer::Artwork {
                    id: page,
                    parent: None,
                    geometry: Geometry {
                        points: vec![
                            Point { x: 0.0, y: 0.0 },
                            Point {
                                x: value.width,
                                y: 0.0,
                            },
                            Point {
                                x: value.width,
                                y: value.height,
                            },
                            Point {
                                x: 0.0,
                                y: value.height,
                            },
                        ],
                    },
                    visibility: LayerVisibility {
                        visible: true,
                        opacity: 1.0,
                    },
                    image,
                },
            );
        }
        let regions = snapshot
            .descendants(page)?
            .map(|entity| {
                let id = entity.id();
                snapshot
                    .component::<SceneRegion>(id)?
                    .map(|region| Self::analysis_region(snapshot, id, region))
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok(Page {
            id: page,
            label: value.label,
            size: PageSize {
                width: value.width,
                height: value.height,
            },
            layers,
            regions,
        })
    }

    fn layer_view(snapshot: &Snapshot, layer: EntityId) -> Result<Layer> {
        let parent = snapshot.parent(layer)?;
        let visibility = Self::layer_visibility(snapshot, layer)?;
        if let Some(group) = snapshot.component::<SceneGroup>(layer)? {
            return Ok(Layer::Group {
                id: layer,
                parent,
                visibility,
                name: group.name,
                role: snapshot
                    .component::<SceneTextGroup>(layer)?
                    .map(|_| GroupRole::Text),
            });
        }
        if let Some(layout) = snapshot.component::<SceneTextLayout>(layer)? {
            let text_layer = snapshot.text_layer(layer)?;
            let content = text_layer.content()?;
            let source = content.source()?.map(|source| SourceText {
                text: source.text.value,
                language: source.language.map(|language| language.to_string()),
            });
            let translation = content.translation()?.map(|translation| Translation {
                text: translation.text.value,
                language: translation.language.map(|language| language.to_string()),
            });
            let translation_progress = snapshot
                .component::<PipelineProgress>(content.id())?
                .and_then(|progress| progress.status(ProgressKind::Translation));
            let translation_complete = match translation_progress {
                Some(ProgressStatus::Complete(_)) => true,
                Some(ProgressStatus::Pending(_)) => false,
                None => translation
                    .as_ref()
                    .is_some_and(|translation| !translation.text.trim().is_empty()),
            };
            let role = content.role()?.map(|role| role.role);
            let source_region = content.source_region()?.map(|region| region.id());
            let automatic_region = text_layer.automatic_target()?.map(|region| region.id());
            let typography = text_layer.typography()?.map(Self::typography_view);
            return Ok(Layer::Text {
                id: layer,
                parent,
                geometry: snapshot
                    .component::<SceneGeometry>(layer)?
                    .map(Self::geometry_view),
                angle_degrees: layout.angle_degrees,
                visibility,
                content: Box::new(TextContent {
                    id: content.id(),
                    source,
                    translation,
                    translation_complete,
                    role,
                    source_region,
                }),
                typography,
                layout: layout.kind,
                automatic_region,
            });
        }
        if let Some(raster) = snapshot.component::<SceneRasterLayer>(layer)? {
            return Ok(Layer::Raster {
                id: layer,
                parent,
                visibility,
                image: Self::asset_id(snapshot, layer, "source")?,
                name: raster.name,
                kind: raster.kind,
            });
        }
        let geometry = snapshot
            .component::<SceneGeometry>(layer)?
            .map(Self::geometry_view)
            .context("image layer has no geometry")?;
        let image = Self::asset_id(snapshot, layer, "source")?
            .context("image layer has no source asset")?;
        Ok(Layer::Image {
            id: layer,
            parent,
            geometry,
            visibility,
            image,
        })
    }

    fn analysis_region(
        snapshot: &Snapshot,
        id: EntityId,
        region: SceneRegion,
    ) -> Result<AnalysisRegion> {
        let geometry = snapshot
            .component::<SceneGeometry>(id)?
            .map(Self::geometry_view)
            .context("analysis region has no geometry")?;
        Ok(AnalysisRegion {
            id,
            parent: snapshot.parent(id)?,
            geometry,
            kind: region.kind.as_str().to_owned(),
            label: region.label,
        })
    }

    fn layer_visibility(snapshot: &Snapshot, layer: EntityId) -> Result<LayerVisibility> {
        Ok(snapshot.component::<SceneVisibility>(layer)?.map_or(
            LayerVisibility {
                visible: true,
                opacity: 1.0,
            },
            |visibility| LayerVisibility {
                visible: visibility.visible,
                opacity: visibility.opacity,
            },
        ))
    }

    fn typography_view(typography: SceneTypography) -> Typography {
        let writing_mode = if matches!(&typography.origin, Origin::User) {
            typography.writing_mode
        } else {
            None
        };
        Typography {
            preferred_font: typography.preferred_font,
            font_weight: typography.font_weight,
            font_style: typography.font_style,
            size: typography.size,
            auto_fit: typography.auto_fit,
            color: typography.color,
            stroke_color: typography.stroke_color,
            stroke_width: typography.stroke_width,
            alignment: typography.alignment,
            writing_mode,
            line_height: typography.line_height,
            letter_spacing: typography.letter_spacing,
        }
    }

    fn geometry_view(geometry: SceneGeometry) -> Geometry {
        Geometry {
            points: geometry
                .points
                .into_iter()
                .map(|point| Point {
                    x: point.x,
                    y: point.y,
                })
                .collect(),
        }
    }

    fn asset_id(snapshot: &Snapshot, entity: EntityId, role: &str) -> Result<Option<String>> {
        Ok(snapshot
            .asset(entity, &AssetRole::new(role)?)?
            .map(|asset| asset.blob.to_string()))
    }

    fn is_layer(snapshot: &Snapshot, entity: EntityId) -> Result<bool> {
        Ok(snapshot.component::<SceneGroup>(entity)?.is_some()
            || snapshot.component::<SceneTextLayout>(entity)?.is_some()
            || snapshot.component::<SceneRasterLayer>(entity)?.is_some()
            || (snapshot.component::<SceneGeometry>(entity)?.is_some()
                && snapshot
                    .asset(entity, &AssetRole::new("source")?)?
                    .is_some()))
    }

    fn is_content_layer(snapshot: &Snapshot, entity: EntityId) -> Result<bool> {
        Ok(
            snapshot.component::<SceneGroup>(entity)?.is_none()
                && Self::is_layer(snapshot, entity)?,
        )
    }

    fn collect_layer_views(
        snapshot: &Snapshot,
        parent: EntityId,
        layers: &mut Vec<Layer>,
    ) -> Result<()> {
        for child in snapshot.children(parent)? {
            if !Self::is_layer(snapshot, child)? {
                continue;
            }
            let group = snapshot.component::<SceneGroup>(child)?.is_some();
            layers.push(Self::layer_view(snapshot, child)?);
            if group {
                Self::collect_layer_views(snapshot, child, layers)?;
            }
        }
        Ok(())
    }

    fn text_content(snapshot: &Snapshot, layer: EntityId) -> Result<EntityId> {
        snapshot
            .text_layer(layer)?
            .content()
            .map(|content| content.id())
            .map_err(Into::into)
    }

    fn placement(siblings: &[EntityId], moving: EntityId, index: usize) -> At {
        siblings
            .iter()
            .copied()
            .filter(|entity| *entity != moving)
            .nth(index)
            .map_or(At::End, At::Before)
    }

    fn unique_roots(snapshot: &Snapshot, entities: Vec<EntityId>) -> Result<Vec<EntityId>> {
        let selected = entities.into_iter().collect::<HashSet<_>>();
        let mut roots = Vec::new();
        for entity in selected.iter().copied() {
            let mut parent = snapshot.parent(entity)?;
            let mut nested = false;
            while let Some(value) = parent {
                if selected.contains(&value) {
                    nested = true;
                    break;
                }
                parent = snapshot.parent(value)?;
            }
            if !nested {
                roots.push(entity);
            }
        }
        roots.sort_unstable();
        Ok(roots)
    }

    fn geometry_from_frame(frame: Frame) -> Result<SceneGeometry> {
        if !frame.x.is_finite()
            || !frame.y.is_finite()
            || !frame.width.is_finite()
            || !frame.height.is_finite()
            || !frame.angle_degrees.is_finite()
            || frame.width <= 0.0
            || frame.height <= 0.0
        {
            bail!("frame must contain finite coordinates and positive dimensions");
        }
        let center_x = f64::from(frame.x + frame.width * 0.5);
        let center_y = f64::from(frame.y + frame.height * 0.5);
        let half_width = f64::from(frame.width) * 0.5;
        let half_height = f64::from(frame.height) * 0.5;
        let (sin, cos) = f64::from(frame.angle_degrees).to_radians().sin_cos();
        Ok(SceneGeometry {
            origin: Origin::User,
            points: [
                (-half_width, -half_height),
                (half_width, -half_height),
                (half_width, half_height),
                (-half_width, half_height),
            ]
            .map(|(x, y)| ScenePoint {
                x: center_x + x * cos - y * sin,
                y: center_y + x * sin + y * cos,
            })
            .into(),
        })
    }
}

fn validate_project_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("project name cannot be empty");
    }
    if name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
    {
        bail!("project name contains characters that cannot be used in a file name");
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
    {
        bail!("project name is reserved by Windows");
    }
    Ok(name.to_owned())
}

fn rasterize_stroke(
    image: &mut RgbaImage,
    mode: RasterStrokeMode,
    color: [u8; 4],
    diameter: f32,
    points: &[ScenePoint],
) {
    let radius = f64::from(diameter) * 0.5;
    for (start, end) in points
        .iter()
        .zip(points.iter().skip(1))
        .chain(points.last().map(|point| (point, point)))
    {
        let left = (start.x.min(end.x) - radius - 0.5).floor().max(0.0) as u32;
        let top = (start.y.min(end.y) - radius - 0.5).floor().max(0.0) as u32;
        let right = (start.x.max(end.x) + radius + 0.5)
            .ceil()
            .min(f64::from(image.width())) as u32;
        let bottom = (start.y.max(end.y) + radius + 0.5)
            .ceil()
            .min(f64::from(image.height())) as u32;
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length_squared = dx * dx + dy * dy;
        for y in top..bottom {
            for x in left..right {
                let px = f64::from(x) + 0.5;
                let py = f64::from(y) + 0.5;
                let t = if length_squared == 0.0 {
                    0.0
                } else {
                    (((px - start.x) * dx + (py - start.y) * dy) / length_squared).clamp(0.0, 1.0)
                };
                let distance =
                    ((px - (start.x + t * dx)).powi(2) + (py - (start.y + t * dy)).powi(2)).sqrt();
                let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0) as f32;
                if coverage == 0.0 {
                    continue;
                }
                let pixel = image.get_pixel_mut(x, y);
                match mode {
                    RasterStrokeMode::Paint => {
                        let source_alpha = f32::from(color[3]) / 255.0 * coverage;
                        let destination_alpha = f32::from(pixel[3]) / 255.0;
                        let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
                        if output_alpha > 0.0 {
                            for channel in 0..3 {
                                let source = f32::from(color[channel]) / 255.0;
                                let destination = f32::from(pixel[channel]) / 255.0;
                                pixel[channel] = (((source * source_alpha
                                    + destination * destination_alpha * (1.0 - source_alpha))
                                    / output_alpha)
                                    * 255.0)
                                    .round() as u8;
                            }
                        }
                        pixel[3] = (output_alpha * 255.0).round() as u8;
                    }
                    RasterStrokeMode::Erase => {
                        pixel[3] = (f32::from(pixel[3]) * (1.0 - coverage)).round() as u8;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use norrix_scene::{Generation, ProducerId, WritingMode};

    use super::*;

    #[tokio::test]
    async fn progress_markers_follow_pages_and_text_entities_and_invalidate_dependents() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let first_page = edit
            .add_page(PageDraft::new("first", 400.0, 300.0), At::End)
            .unwrap();
        let second_page = edit
            .add_page(PageDraft::new("second", 400.0, 300.0), At::End)
            .unwrap();
        session.commit(edit.finish().unwrap()).await.unwrap();
        let mut project = Project::new(session, "progress-test".to_owned());

        let (_, first_layer) = project
            .add_point_text(first_page, Point { x: 10.0, y: 10.0 })
            .await
            .unwrap();
        let (_, second_layer) = project
            .add_point_text(first_page, Point { x: 10.0, y: 50.0 })
            .await
            .unwrap();
        project
            .set_source_text(first_layer, "first source".to_owned())
            .await
            .unwrap();
        project
            .set_source_text(second_layer, "second source".to_owned())
            .await
            .unwrap();

        let commit = project
            .set_progress_markers(
                vec![first_page, second_page],
                None,
                ProgressKind::Detection,
                true,
            )
            .await
            .unwrap();
        project.record_commit(&commit);
        let commit = project
            .set_progress_markers(
                Vec::new(),
                Some(first_layer),
                ProgressKind::Translation,
                true,
            )
            .await
            .unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::pages(&project.snapshot())
                .unwrap()
                .iter()
                .find(|page| page.id == first_page)
                .map(|page| (page.translated_texts, page.total_texts)),
            Some((1, 2))
        );

        let commit = project
            .set_translation(second_layer, Some("manual translation".to_owned()))
            .await
            .unwrap();
        project.record_commit(&commit);
        let commit = project
            .set_progress_markers(vec![first_page], None, ProgressKind::Inpainting, true)
            .await
            .unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::pages(&project.snapshot())
                .unwrap()
                .iter()
                .find(|page| page.id == first_page)
                .map(|page| (
                    page.translated_texts,
                    page.total_texts,
                    page.inpainting_complete
                )),
            Some((2, 2, true))
        );

        let commit = project
            .set_source_text(second_layer, "changed source".to_owned())
            .await
            .unwrap();
        project.record_commit(&commit);
        let commit = project.move_page(first_page, 1).await.unwrap();
        project.record_commit(&commit);
        let first_summary = Project::pages(&project.snapshot())
            .unwrap()
            .into_iter()
            .find(|page| page.id == first_page)
            .unwrap();
        assert_eq!(
            (
                first_summary.translated_texts,
                first_summary.total_texts,
                first_summary.inpainting_complete,
                first_summary.detection_complete,
            ),
            (1, 2, false, true)
        );

        let commit = project.delete_layers(vec![second_layer]).await.unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::pages(&project.snapshot())
                .unwrap()
                .into_iter()
                .find(|page| page.id == first_page)
                .map(|page| {
                    (
                        page.translated_texts,
                        page.total_texts,
                        page.translation_complete,
                    )
                }),
            Some((1, 1, true))
        );
        let (_, new_layer) = project
            .add_point_text(first_page, Point { x: 30.0, y: 30.0 })
            .await
            .unwrap();
        let page_summary = Project::pages(&project.snapshot())
            .unwrap()
            .into_iter()
            .find(|page| page.id == first_page)
            .unwrap();
        assert_eq!(
            (
                page_summary.translated_texts,
                page_summary.total_texts,
                page_summary.translation_complete,
            ),
            (1, 2, false)
        );
        assert_ne!(first_layer, new_layer);
    }

    #[tokio::test]
    async fn page_translation_markers_cover_text_layers_and_empty_pages() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page-translation-markers.khrproj");
        let (text_page, empty_page, first_layer) = {
            let mut project = Project::create("page-translation-markers".to_owned(), path.clone())
                .await
                .unwrap();
            let mut edit = project.snapshot().edit();
            let text_page = edit
                .add_page(PageDraft::new("text", 400.0, 300.0), At::End)
                .unwrap();
            let empty_page = edit
                .add_page(PageDraft::new("empty", 400.0, 300.0), At::End)
                .unwrap();
            project
                .session
                .commit(edit.finish().unwrap())
                .await
                .unwrap();

            let (_, first_layer) = project
                .add_point_text(text_page, Point { x: 10.0, y: 10.0 })
                .await
                .unwrap();
            let (_, second_layer) = project
                .add_point_text(text_page, Point { x: 10.0, y: 50.0 })
                .await
                .unwrap();
            let commit = project
                .set_progress_markers(vec![text_page], None, ProgressKind::Translation, true)
                .await
                .unwrap();
            project.record_commit(&commit);

            let snapshot = project.snapshot();
            for layer in [first_layer, second_layer] {
                let content = snapshot.text_layer(layer).unwrap().content().unwrap().id();
                assert_eq!(
                    snapshot
                        .component::<PipelineProgress>(content)
                        .unwrap()
                        .and_then(|progress| progress.status(ProgressKind::Translation)),
                    Some(ProgressStatus::Complete(ProgressOrigin::Manual))
                );
            }
            assert_eq!(
                Project::pages(&snapshot)
                    .unwrap()
                    .iter()
                    .find(|page| page.id == text_page)
                    .map(|page| (
                        page.translation_complete,
                        page.translated_texts,
                        page.total_texts
                    )),
                Some((true, 2, 2))
            );
            let commit = project
                .set_progress_markers(vec![empty_page], None, ProgressKind::Translation, true)
                .await
                .unwrap();
            project.record_commit(&commit);
            assert_eq!(
                project
                    .snapshot()
                    .component::<PipelineProgress>(empty_page)
                    .unwrap()
                    .and_then(|progress| progress.status(ProgressKind::Translation)),
                Some(ProgressStatus::Complete(ProgressOrigin::Manual))
            );
            assert!(
                Project::pages(&project.snapshot())
                    .unwrap()
                    .iter()
                    .find(|page| page.id == empty_page)
                    .unwrap()
                    .translation_complete
            );

            let commit = project
                .set_progress_markers(vec![text_page], None, ProgressKind::Translation, false)
                .await
                .unwrap();
            project.record_commit(&commit);
            assert_eq!(
                Project::translation_progress(&project.snapshot(), text_page).unwrap(),
                (0, 2)
            );
            let commit = project
                .set_progress_markers(
                    Vec::new(),
                    Some(first_layer),
                    ProgressKind::Translation,
                    true,
                )
                .await
                .unwrap();
            project.record_commit(&commit);
            assert_eq!(
                Project::translation_progress(&project.snapshot(), text_page).unwrap(),
                (1, 2)
            );
            (text_page, empty_page, first_layer)
        };

        let reopened = Project::open("page-translation-markers".to_owned(), path)
            .await
            .unwrap();
        let snapshot = reopened.snapshot();
        let content = snapshot
            .text_layer(first_layer)
            .unwrap()
            .content()
            .unwrap()
            .id();
        assert_eq!(
            Project::translation_progress(&snapshot, text_page).unwrap(),
            (1, 2)
        );
        assert_eq!(
            snapshot
                .component::<PipelineProgress>(content)
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation)),
            Some(ProgressStatus::Complete(ProgressOrigin::Manual))
        );
        assert_eq!(
            snapshot
                .component::<PipelineProgress>(empty_page)
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation)),
            Some(ProgressStatus::Complete(ProgressOrigin::Manual))
        );
    }

    #[tokio::test]
    async fn legacy_translation_counts_only_nonempty_text_and_ignores_visual_edits() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(PageDraft::new("legacy translations", 400.0, 300.0), At::End)
            .unwrap();
        session.commit(edit.finish().unwrap()).await.unwrap();
        let mut project = Project::new(session, "legacy-translation-progress".to_owned());

        let mut layers = Vec::new();
        for index in 0..4 {
            let (_, layer) = project
                .add_point_text(
                    page,
                    Point {
                        x: 10.0,
                        y: 20.0 + index as f64 * 40.0,
                    },
                )
                .await
                .unwrap();
            project
                .set_source_text(layer, format!("source {index}"))
                .await
                .unwrap();
            layers.push(layer);
        }

        let snapshot = project.snapshot();
        let contents = layers
            .iter()
            .map(|layer| snapshot.text_layer(*layer).unwrap().content().unwrap().id())
            .collect::<Vec<_>>();
        let patch = snapshot
            .patch(|edit| {
                edit.set(
                    contents[0],
                    &SceneTranslation {
                        text: Authored::user("legacy translation".to_owned()),
                        language: None,
                    },
                )?;
                edit.set(
                    contents[1],
                    &SceneTranslation {
                        text: Authored::user(String::new()),
                        language: None,
                    },
                )?;
                edit.set(
                    contents[2],
                    &SceneTranslation {
                        text: Authored::user(" \t\n".to_owned()),
                        language: None,
                    },
                )?;
                Ok(())
            })
            .unwrap();
        let commit = project.commit(patch).await.unwrap();
        project.record_commit(&commit);

        assert_eq!(
            Project::translation_progress(&project.snapshot(), page).unwrap(),
            (1, 4)
        );

        let parent = project.snapshot().parent(layers[0]).unwrap().unwrap();
        let commit = project.move_layer(layers[0], parent, 3).await.unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::translation_progress(&project.snapshot(), page).unwrap(),
            (1, 4)
        );

        let commit = project
            .set_geometry(vec![GeometryUpdate {
                layer: layers[0],
                points: Some(vec![
                    Point { x: 10.0, y: 10.0 },
                    Point { x: 120.0, y: 10.0 },
                    Point { x: 120.0, y: 40.0 },
                    Point { x: 10.0, y: 40.0 },
                ]),
            }])
            .await
            .unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::translation_progress(&project.snapshot(), page).unwrap(),
            (1, 4)
        );

        let commit = project
            .set_source_text(layers[0], "changed source".to_owned())
            .await
            .unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::translation_progress(&project.snapshot(), page).unwrap(),
            (0, 4)
        );
        assert_eq!(
            project
                .snapshot()
                .component::<PipelineProgress>(contents[0])
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation)),
            Some(ProgressStatus::Pending(ProgressOrigin::Automatic))
        );

        let commit = project
            .set_translation(layers[0], Some(" \t\n".to_owned()))
            .await
            .unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::translation_progress(&project.snapshot(), page).unwrap(),
            (0, 4)
        );
        assert_eq!(
            project
                .snapshot()
                .component::<PipelineProgress>(contents[0])
                .unwrap()
                .and_then(|progress| progress.status(ProgressKind::Translation)),
            Some(ProgressStatus::Pending(ProgressOrigin::Manual))
        );

        let commit = project
            .set_text_content(vec![TextContentUpdate {
                layer: layers[0],
                value: TextFieldUpdate::Translation(Some(String::new())),
            }])
            .await
            .unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::translation_progress(&project.snapshot(), page).unwrap(),
            (0, 4)
        );

        let commit = project
            .set_text_content(vec![TextContentUpdate {
                layer: layers[0],
                value: TextFieldUpdate::Translation(Some("manual translation".to_owned())),
            }])
            .await
            .unwrap();
        project.record_commit(&commit);
        assert_eq!(
            Project::translation_progress(&project.snapshot(), page).unwrap(),
            (1, 4)
        );
    }

    #[test]
    fn opening_a_legacy_project_copies_it_and_preserves_the_original() {
        let documents = tempfile::tempdir().unwrap();
        let legacy_root = documents.path().join("Koharu");
        let norrix_root = documents.path().join("Norrix");
        let legacy_project = legacy_root.join("Example.khrproj");
        std::fs::create_dir_all(&legacy_project).unwrap();
        std::fs::write(legacy_project.join("state-a.khr"), b"legacy project").unwrap();
        let library = ProjectLibrary {
            root: norrix_root.clone(),
            legacy_root: legacy_root.clone(),
        };
        std::fs::create_dir_all(&library.root).unwrap();

        let listed = library.list().unwrap();
        let resolved = library.resolve_for_open("Example").unwrap();
        let norrix_project = norrix_root.join("Example.khrproj");

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Example");
        assert_eq!(resolved.1, norrix_project);
        assert_eq!(
            std::fs::read(legacy_project.join("state-a.khr")).unwrap(),
            b"legacy project"
        );
        assert_eq!(
            std::fs::read(norrix_project.join("state-a.khr")).unwrap(),
            b"legacy project"
        );

        std::fs::write(norrix_project.join("state-a.khr"), b"Norrix changes").unwrap();
        let repeated = library.resolve_for_open("Example").unwrap();
        assert_eq!(repeated.1, norrix_project);
        assert_eq!(
            std::fs::read(norrix_project.join("state-a.khr")).unwrap(),
            b"Norrix changes"
        );
        assert_eq!(
            std::fs::read(legacy_project.join("state-a.khr")).unwrap(),
            b"legacy project"
        );
    }

    #[test]
    fn norrix_project_wins_name_conflicts_without_changing_legacy_project() {
        let documents = tempfile::tempdir().unwrap();
        let legacy_root = documents.path().join("Koharu");
        let norrix_root = documents.path().join("Norrix");
        let legacy_project = legacy_root.join("Example.khrproj");
        let norrix_project = norrix_root.join("Example.khrproj");
        std::fs::create_dir_all(&legacy_project).unwrap();
        std::fs::create_dir_all(&norrix_project).unwrap();
        std::fs::write(legacy_project.join("state-a.khr"), b"legacy").unwrap();
        std::fs::write(norrix_project.join("state-a.khr"), b"norrix").unwrap();
        let library = ProjectLibrary {
            root: norrix_root,
            legacy_root,
        };

        let listed = library.list().unwrap();
        let resolved = library.resolve_for_open("Example").unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(resolved.1, library.root.join("Example.khrproj"));
        assert_eq!(
            std::fs::read(legacy_project.join("state-a.khr")).unwrap(),
            b"legacy"
        );
        assert_eq!(
            std::fs::read(norrix_project.join("state-a.khr")).unwrap(),
            b"norrix"
        );
    }

    #[tokio::test]
    async fn creating_a_project_does_not_shadow_a_legacy_project() {
        let documents = tempfile::tempdir().unwrap();
        let legacy_root = documents.path().join("Koharu");
        let legacy_project = legacy_root.join("Example.khrproj");
        std::fs::create_dir_all(&legacy_project).unwrap();
        std::fs::write(legacy_project.join("state-a.khr"), b"legacy").unwrap();
        let library = ProjectLibrary {
            root: documents.path().join("Norrix"),
            legacy_root,
        };
        std::fs::create_dir_all(&library.root).unwrap();

        assert!(library.create("Example").await.is_err());
        assert_eq!(
            std::fs::read(legacy_project.join("state-a.khr")).unwrap(),
            b"legacy"
        );
        assert!(!library.root.join("Example.khrproj").exists());
    }

    #[test]
    fn deleting_removes_legacy_projects_and_all_duplicate_copies() {
        let documents = tempfile::tempdir().unwrap();
        let legacy_root = documents.path().join("Koharu");
        let legacy_project = legacy_root.join("Example.khrproj");
        let norrix_root = documents.path().join("Norrix");
        let norrix_project = norrix_root.join("Example.khrproj");
        std::fs::create_dir_all(&legacy_project).unwrap();
        std::fs::create_dir_all(&norrix_project).unwrap();
        std::fs::write(legacy_project.join("state-a.khr"), b"legacy").unwrap();
        let library = ProjectLibrary {
            root: norrix_root,
            legacy_root,
        };

        library.delete("Example").unwrap();
        assert!(!legacy_project.exists());
        assert!(!norrix_project.exists());
        assert!(library.delete("Example").is_err());
    }

    #[test]
    fn deleting_a_missing_project_returns_an_error() {
        let documents = tempfile::tempdir().unwrap();
        let library = ProjectLibrary {
            root: documents.path().join("Norrix"),
            legacy_root: documents.path().join("Koharu"),
        };
        std::fs::create_dir_all(&library.root).unwrap();

        assert!(library.delete("Example").is_err());
    }

    #[test]
    fn only_user_authored_direction_is_projected_as_an_override() {
        let mut typography = SceneTypography {
            origin: Origin::Generated(Generation::new(
                ProducerId::new("dev.koharu.pipeline.detection").expect("valid producer"),
            )),
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
            writing_mode: Some(WritingMode::Vertical),
            line_height: None,
            letter_spacing: None,
            extensions: Default::default(),
        };

        assert_eq!(
            Project::typography_view(typography.clone()).writing_mode,
            None
        );
        typography.origin = Origin::User;
        assert_eq!(
            Project::typography_view(typography).writing_mode,
            Some(WritingMode::Vertical)
        );
    }

    #[tokio::test]
    async fn axis_resizes_pin_rendered_auto_fit_size_and_diagonal_resizes_release_it() {
        let mut session = Session::memory().await.unwrap();
        let mut edit = session.snapshot().edit();
        let page = edit
            .add_page(PageDraft::new("resize", 400.0, 300.0), At::End)
            .unwrap();
        let content = edit.add_text_content(page, At::End).unwrap();
        edit.set(
            content,
            &SceneSourceText {
                text: Authored::user("Source sentence".to_owned()),
                language: None,
            },
        )
        .unwrap();
        edit.set(
            content,
            &SceneTranslation {
                text: Authored::user("A sentence that fits at a stable size".to_owned()),
                language: None,
            },
        )
        .unwrap();
        let element = edit
            .add_text_layer(
                page,
                At::End,
                content,
                &SceneTextLayout {
                    origin: Origin::User,
                    kind: TextLayoutKind::Paragraph,
                    angle_degrees: Some(0.0),
                },
            )
            .unwrap();
        edit.set(element, &SceneGeometry::rectangle(20.0, 30.0, 160.0, 80.0))
            .unwrap();
        edit.set(
            element,
            &SceneTypography {
                origin: Origin::User,
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
        session.commit(edit.finish().unwrap()).await.unwrap();
        let mut project = Project::new(session, "resize-test".to_owned());

        for (frame, diagonal) in [
            (
                Frame {
                    x: 20.0,
                    y: 30.0,
                    width: 240.0,
                    height: 80.0,
                    angle_degrees: 0.0,
                },
                false,
            ),
            (
                Frame {
                    x: 20.0,
                    y: 30.0,
                    width: 240.0,
                    height: 120.0,
                    angle_degrees: 0.0,
                },
                false,
            ),
        ] {
            let commit = project
                .set_geometries([(
                    element,
                    Project::geometry_from_frame(frame).unwrap(),
                    frame.angle_degrees,
                    diagonal,
                    18.5,
                )])
                .await
                .unwrap();
            project.record_commit(&commit);
            let typography = project
                .snapshot()
                .component::<SceneTypography>(element)
                .unwrap()
                .unwrap();
            assert!(typography.auto_fit);
            assert_eq!(typography.auto_fit_size, Some(18.5));
        }

        let diagonal_frame = Frame {
            x: 20.0,
            y: 30.0,
            width: 300.0,
            height: 150.0,
            angle_degrees: 0.0,
        };
        let commit = project
            .set_geometries([(
                element,
                Project::geometry_from_frame(diagonal_frame).unwrap(),
                diagonal_frame.angle_degrees,
                true,
                18.5,
            )])
            .await
            .unwrap();
        project.record_commit(&commit);
        let mut typography = project
            .snapshot()
            .component::<SceneTypography>(element)
            .unwrap()
            .unwrap();
        assert!(typography.auto_fit);
        assert_eq!(typography.auto_fit_size, None);

        typography.auto_fit = false;
        typography.size = Some(22.0);
        let commit = project
            .commit(
                project
                    .snapshot()
                    .patch(|edit| edit.set(element, &typography))
                    .unwrap(),
            )
            .await
            .unwrap();
        project.record_commit(&commit);
        let commit = project
            .set_geometries([(
                element,
                Project::geometry_from_frame(Frame {
                    x: 20.0,
                    y: 30.0,
                    width: 360.0,
                    height: 180.0,
                    angle_degrees: 0.0,
                })
                .unwrap(),
                0.0,
                true,
                18.5,
            )])
            .await
            .unwrap();
        project.record_commit(&commit);
        let typography = project
            .snapshot()
            .component::<SceneTypography>(element)
            .unwrap()
            .unwrap();
        assert!(!typography.auto_fit);
        assert_eq!(typography.size, Some(22.0));
        assert_eq!(typography.auto_fit_size, None);
    }

    #[tokio::test]
    async fn pipeline_commit_rebases_or_yields_to_the_manual_edit() {
        let mut session = Session::memory().await.unwrap();
        let mut setup = session.snapshot().edit();
        let pipeline_page = setup
            .add_page(PageDraft::new("pipeline", 100.0, 100.0), At::End)
            .unwrap();
        let manual_page = setup
            .add_page(PageDraft::new("manual", 100.0, 100.0), At::End)
            .unwrap();
        session.commit(setup.finish().unwrap()).await.unwrap();
        let mut project = Project::new(session, "test".to_owned());

        let base = project.snapshot();
        let pipeline = base
            .patch(|edit| {
                edit.set_page(
                    pipeline_page,
                    PageDraft::new("pipeline result", 100.0, 100.0),
                )
            })
            .unwrap();
        let manual = base
            .patch(|edit| edit.set_page(manual_page, PageDraft::new("manual edit", 100.0, 100.0)))
            .unwrap();
        project.commit(manual).await.unwrap();

        let commit = project.commit_rebased(pipeline).await.unwrap().unwrap();
        assert_eq!(
            commit
                .snapshot
                .page(pipeline_page)
                .unwrap()
                .page()
                .unwrap()
                .label,
            "pipeline result"
        );
        assert_eq!(
            commit
                .snapshot
                .page(manual_page)
                .unwrap()
                .page()
                .unwrap()
                .label,
            "manual edit"
        );

        let base = project.snapshot();
        let pipeline = base
            .patch(|edit| {
                edit.set_page(
                    pipeline_page,
                    PageDraft::new("stale pipeline", 100.0, 100.0),
                )
            })
            .unwrap();
        let manual = base
            .patch(|edit| {
                edit.set_page(pipeline_page, PageDraft::new("latest manual", 100.0, 100.0))
            })
            .unwrap();
        project.commit(manual).await.unwrap();

        assert!(project.commit_rebased(pipeline).await.unwrap().is_none());
        assert_eq!(
            project
                .snapshot()
                .page(pipeline_page)
                .unwrap()
                .page()
                .unwrap()
                .label,
            "latest manual"
        );
    }

    #[tokio::test]
    async fn created_text_box_and_content_survive_reopening_the_project() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("validation.khrproj");
        let (page, layer) = {
            let mut project = Project::create("validation".to_owned(), path.clone())
                .await
                .unwrap();
            let mut setup = project.snapshot().edit();
            let page = setup
                .add_page(PageDraft::new("page", 1200.0, 2296.0), At::End)
                .unwrap();
            project
                .session
                .commit(setup.finish().unwrap())
                .await
                .unwrap();
            project.active_page = Some(page);

            let (created, layer) = project
                .add_text_box(
                    page,
                    Frame {
                        x: 24.0,
                        y: 32.0,
                        width: 320.0,
                        height: 120.0,
                        angle_degrees: 0.0,
                    },
                )
                .await
                .unwrap();
            assert_eq!(project.page_for_layer(layer).unwrap(), page);
            let mut add_second_page = project.snapshot().edit();
            let second_page = add_second_page
                .add_page(PageDraft::new("second page", 1200.0, 2296.0), At::End)
                .unwrap();
            project
                .session
                .commit(add_second_page.finish().unwrap())
                .await
                .unwrap();
            let (_, second_layer) = project
                .add_text_box(
                    second_page,
                    Frame {
                        x: 24.0,
                        y: 32.0,
                        width: 320.0,
                        height: 120.0,
                        angle_degrees: 0.0,
                    },
                )
                .await
                .unwrap();
            assert_eq!(project.active_page(), Some(page));
            assert_eq!(project.page_for_layer(second_layer).unwrap(), second_page);
            project.record_commit(&created);
            let created_snapshot = project.snapshot();
            let content = created_snapshot
                .text_layer(layer)
                .unwrap()
                .content()
                .unwrap();
            assert_eq!(content.translation().unwrap().unwrap().text.value, "");
            let updated = project
                .set_source_text(layer, "Persisted new text".to_owned())
                .await
                .unwrap();
            project.record_commit(&updated);
            let styled = project
                .set_typography(vec![TypographyUpdate {
                    layer,
                    typography: Typography {
                        preferred_font: None,
                        font_weight: None,
                        font_style: None,
                        size: None,
                        auto_fit: true,
                        color: None,
                        stroke_color: None,
                        stroke_width: None,
                        alignment: None,
                        writing_mode: None,
                        line_height: Some(1.75),
                        letter_spacing: Some(60.0),
                    },
                }])
                .await
                .unwrap();
            project.record_commit(&styled);
            let resized = project
                .set_geometries([(
                    layer,
                    Project::geometry_from_frame(Frame {
                        x: 24.0,
                        y: 32.0,
                        width: 640.0,
                        height: 240.0,
                        angle_degrees: 0.0,
                    })
                    .unwrap(),
                    0.0,
                    true,
                    36.0,
                )])
                .await
                .unwrap();
            project.record_commit(&resized);
            (page, layer)
        };

        let reopened = Project::open("validation".to_owned(), path).await.unwrap();
        let snapshot = reopened.snapshot();
        let page_view = Project::page(&snapshot, page).unwrap();
        let Layer::Text {
            id: _,
            content,
            parent,
            typography,
            ..
        } = page_view
            .layers
            .iter()
            .find(|candidate| matches!(candidate, Layer::Text { id, .. } if *id == layer))
            .expect("the created text layer should be in the reopened page")
        else {
            panic!("the created entity should remain a text layer");
        };
        assert!(parent.is_some());
        assert_eq!(
            content.source.as_ref().map(|source| source.text.as_str()),
            Some("Persisted new text")
        );
        assert_eq!(typography.as_ref().unwrap().line_height, Some(1.75));
        assert_eq!(typography.as_ref().unwrap().letter_spacing, Some(60.0));
        assert!(typography.as_ref().unwrap().auto_fit);
        assert_eq!(typography.as_ref().unwrap().size, None);
        assert_eq!(
            snapshot
                .component::<SceneTypography>(layer)
                .unwrap()
                .unwrap()
                .auto_fit_size,
            None
        );
        assert_eq!(typography.as_ref().unwrap().line_height, Some(1.75));
        assert_eq!(snapshot.parent(content.id).unwrap(), Some(page));
    }

    #[test]
    fn raster_strokes_are_continuous_and_erasable() {
        let mut image = RgbaImage::new(32, 16);
        let points = [
            ScenePoint { x: 4.0, y: 8.0 },
            ScenePoint { x: 28.0, y: 8.0 },
        ];
        rasterize_stroke(
            &mut image,
            RasterStrokeMode::Paint,
            [210, 40, 20, 255],
            5.0,
            &points,
        );
        for x in 4..28 {
            assert_eq!(image.get_pixel(x, 8).0, [210, 40, 20, 255]);
        }

        rasterize_stroke(
            &mut image,
            RasterStrokeMode::Erase,
            [0, 0, 0, 0],
            5.0,
            &[ScenePoint { x: 16.0, y: 8.0 }],
        );
        assert_eq!(image.get_pixel(16, 8)[3], 0);
        assert_eq!(image.get_pixel(4, 8)[3], 255);

        let mut white = RgbaImage::new(4, 4);
        rasterize_stroke(
            &mut white,
            RasterStrokeMode::Paint,
            [255, 255, 255, 255],
            3.0,
            &[ScenePoint { x: 2.0, y: 2.0 }],
        );
        assert_eq!(white.get_pixel(2, 2).0, [255, 255, 255, 255]);
    }
}
