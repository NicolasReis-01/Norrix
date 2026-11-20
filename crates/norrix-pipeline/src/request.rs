use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{ProgressSink, Scope, Stage};
use norrix_scene::EntityId;

#[derive(Clone, Debug)]
pub struct InpaintingMask {
    pub page: EntityId,
    pub png: Arc<[u8]>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Operation {
    #[default]
    Full,
    Through {
        stage: Stage,
    },
    Only {
        stage: Stage,
    },
    Stages {
        stages: Vec<Stage>,
    },
}

impl Operation {
    pub(crate) fn stages(&self) -> Result<Vec<Stage>> {
        let stages = match self {
            Self::Full => Stage::ALL.to_vec(),
            Self::Through {
                stage: Stage::Detection,
            } => vec![Stage::Detection],
            Self::Through { stage: Stage::Ocr } => vec![Stage::Detection, Stage::Ocr],
            Self::Through {
                stage: Stage::Translation,
            } => {
                vec![Stage::Detection, Stage::Ocr, Stage::Translation]
            }
            Self::Through {
                stage: Stage::Inpainting,
            } => vec![Stage::Detection, Stage::Inpainting],
            Self::Only { stage } => vec![*stage],
            Self::Stages { stages } => Stage::ALL
                .into_iter()
                .filter(|stage| stages.contains(stage))
                .collect(),
        };
        if stages.is_empty() {
            bail!("at least one pipeline stage must be selected");
        }
        Ok(stages)
    }
}

#[derive(Clone)]
pub struct Request {
    pub operation: Operation,
    pub scope: Scope,
    pub stop: StopToken,
    pub progress: Option<ProgressSink>,
    pub inpainting_mask: Option<InpaintingMask>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            operation: Operation::Full,
            scope: Scope::Project,
            stop: StopToken::default(),
            progress: None,
            inpainting_mask: None,
        }
    }
}

#[derive(Default)]
struct StopState {
    stopped: AtomicBool,
    changed: Notify,
    reason: OnceLock<&'static str>,
}

#[derive(Clone, Default)]
pub struct StopToken(Arc<StopState>);

impl StopToken {
    pub fn stop(&self) {
        self.stop_with_reason("unspecified");
    }

    pub fn stop_with_reason(&self, reason: &'static str) {
        let _ = self.0.reason.set(reason);
        if !self.0.stopped.swap(true, Ordering::AcqRel) {
            self.0.changed.notify_waiters();
        }
    }

    #[must_use]
    pub fn cancellation_reason(&self) -> Option<&'static str> {
        self.0.reason.get().copied()
    }

    #[must_use]
    pub fn stopped(&self) -> bool {
        self.0.stopped.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        let changed = self.0.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if self.stopped() {
            return;
        }
        changed.await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::StopToken;

    #[tokio::test]
    async fn cancellation_waiter_is_released_once_stopped() {
        let stop = StopToken::default();
        let waiting = stop.cancelled();
        stop.stop();

        tokio::time::timeout(Duration::from_millis(50), waiting)
            .await
            .expect("stop should wake cancellation waiters");
        assert!(stop.stopped());
    }

    #[tokio::test]
    async fn cancellation_waiter_observes_an_already_stopped_token() {
        let stop = StopToken::default();
        stop.stop_with_reason("user_requested");

        tokio::time::timeout(Duration::from_millis(50), stop.cancelled())
            .await
            .expect("an already stopped token should not wait");
        assert_eq!(stop.cancellation_reason(), Some("user_requested"));
    }

    #[test]
    fn cancellation_reason_is_first_writer_wins() {
        let stop = StopToken::default();
        stop.stop_with_reason("user_requested");
        stop.stop_with_reason("application_shutdown");
        assert_eq!(stop.cancellation_reason(), Some("user_requested"));
    }

    #[test]
    fn cancellation_sources_remain_distinguishable() {
        let user_stop = StopToken::default();
        user_stop.stop_with_reason("user_requested");

        let window_close = StopToken::default();
        window_close.stop_with_reason("window_close_requested");

        assert_eq!(user_stop.cancellation_reason(), Some("user_requested"));
        assert_eq!(
            window_close.cancellation_reason(),
            Some("window_close_requested")
        );
    }
}
