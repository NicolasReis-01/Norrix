use revision::revisioned;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{
    Result,
    component::{Component, ValidationContext},
};

#[revisioned(revision = 1)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProgressOrigin {
    Automatic,
    Manual,
}

#[revisioned(revision = 1)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "state", content = "origin", rename_all = "snake_case")]
pub enum ProgressStatus {
    Complete(ProgressOrigin),
    Pending(ProgressOrigin),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProgressKind {
    Detection,
    Ocr,
    Translation,
    Inpainting,
}

#[revisioned(revision = 1)]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Type)]
pub struct PipelineProgress {
    pub detection: Option<ProgressStatus>,
    pub ocr: Option<ProgressStatus>,
    pub translation: Option<ProgressStatus>,
    pub inpainting: Option<ProgressStatus>,
}

impl PipelineProgress {
    #[must_use]
    pub fn is_complete(&self, kind: ProgressKind) -> bool {
        matches!(self.status(kind), Some(ProgressStatus::Complete(_)))
    }

    #[must_use]
    pub fn status(&self, kind: ProgressKind) -> Option<ProgressStatus> {
        match kind {
            ProgressKind::Detection => self.detection,
            ProgressKind::Ocr => self.ocr,
            ProgressKind::Translation => self.translation,
            ProgressKind::Inpainting => self.inpainting,
        }
    }

    pub fn set(&mut self, kind: ProgressKind, status: Option<ProgressStatus>) {
        match kind {
            ProgressKind::Detection => self.detection = status,
            ProgressKind::Ocr => self.ocr = status,
            ProgressKind::Translation => self.translation = status,
            ProgressKind::Inpainting => self.inpainting = status,
        }
    }
}

impl Component for PipelineProgress {
    const KIND: &'static str = "dev.norrix.pipeline.progress";

    fn validate(&self, _context: &ValidationContext<'_>) -> Result<()> {
        Ok(())
    }
}
