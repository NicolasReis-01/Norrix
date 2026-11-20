use revision::revisioned;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{
    Error, Result,
    component::{Component, ValidationContext},
};

use super::Origin;

#[revisioned(revision = 1)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct OriginalFontEstimate {
    pub origin: Origin,
    pub size: f32,
}

impl Component for OriginalFontEstimate {
    const KIND: &'static str = "dev.norrix.text.original-font-estimate";

    fn validate(&self, _context: &ValidationContext<'_>) -> Result<()> {
        self.origin.validate()?;
        if self.size.is_finite() && self.size > 0.0 {
            Ok(())
        } else {
            Err(Error::invalid(
                "original font estimate must be positive and finite",
            ))
        }
    }

    fn origin(&self) -> Option<&Origin> {
        Some(&self.origin)
    }

    fn set_origin(&mut self, origin: Origin) -> bool {
        self.origin = origin;
        true
    }
}
