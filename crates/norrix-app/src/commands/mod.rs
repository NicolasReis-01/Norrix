pub(crate) mod agent;
pub(crate) mod canvas;
pub(crate) mod editing;
pub(crate) mod fonts;
pub(crate) mod import;
pub(crate) mod lifecycle;
pub(crate) mod output;
pub(crate) mod preferences;
pub(crate) mod processing;
pub(crate) mod project;

use parking_lot::Mutex;
use serde::Serialize;
use specta::Type;
use tauri::ipc::{Channel, IpcResponse};

use norrix_desktop::CanvasState;

#[derive(Debug, Type)]
#[specta(transparent)]
pub(crate) struct Error(#[specta(type = String)] anyhow::Error);

impl<E> From<E> for Error
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("{:#}", self.0))
    }
}

pub(crate) trait ChannelExt<T> {
    fn publish(&self, value: T);
}

impl<T: IpcResponse> ChannelExt<T> for Mutex<Option<Channel<T>>> {
    fn publish(&self, value: T) {
        let mut channel = self.lock();
        if channel
            .as_ref()
            .is_some_and(|channel| channel.send(value).is_err())
        {
            channel.take();
        }
    }
}

pub(crate) fn publish_canvas(
    channel: &Mutex<Option<Channel<CanvasState>>>,
    state: CanvasState,
    job_id: Option<&str>,
    processed_page: Option<norrix_scene::EntityId>,
) {
    let trace_id = state.trace_id.clone();
    let page = state.page;
    let revision = state.revision;
    let generation = state.generation;
    let mut registered_channel = channel.lock();
    let Some(sender) = registered_channel.as_ref() else {
        tracing::warn!(
            event = "canvas_state_published",
            trace_id,
            job_id = job_id.unwrap_or("unavailable"),
            processed_page = %processed_page.map_or_else(|| "unavailable".to_owned(), |id| id.to_string()),
            page = %page.map_or_else(|| "none".to_owned(), |id| id.to_string()),
            revision = %revision.map_or_else(|| "none".to_owned(), |revision| revision.to_string()),
            generation,
            subscribed = false,
            result = "not_sent",
            reason = "no frontend canvas channel is registered",
        );
        return;
    };
    match sender.send(state) {
        Ok(()) => tracing::info!(
            event = "canvas_state_published",
            trace_id,
            job_id = job_id.unwrap_or("unavailable"),
            processed_page = %processed_page.map_or_else(|| "unavailable".to_owned(), |id| id.to_string()),
            page = %page.map_or_else(|| "none".to_owned(), |id| id.to_string()),
            revision = %revision.map_or_else(|| "none".to_owned(), |revision| revision.to_string()),
            generation,
            subscribed = true,
            result = "sent",
            confirmation = "transport_accepted_not_frontend_ack",
        ),
        Err(error) => {
            tracing::error!(
                event = "canvas_state_published",
                trace_id,
                job_id = job_id.unwrap_or("unavailable"),
                processed_page = %processed_page.map_or_else(|| "unavailable".to_owned(), |id| id.to_string()),
                page = %page.map_or_else(|| "none".to_owned(), |id| id.to_string()),
                revision = %revision.map_or_else(|| "none".to_owned(), |revision| revision.to_string()),
                generation,
                subscribed = true,
                result = "send_failed",
                error = %error,
            );
            registered_channel.take();
        }
    }
}

pub fn bindings() -> tauri_specta::Builder<tauri_runtime_cef::CefRuntime> {
    use tauri_specta::{Builder, ErrorHandlingMode, collect_commands};

    Builder::new()
        .commands(collect_commands![
            agent::get_agent_status,
            agent::login_agent,
            agent::logout_agent,
            agent::save_agent_config,
            agent::run_agent,
            agent::cancel_agent,
            lifecycle::subscribe,
            lifecycle::get_project,
            lifecycle::get_pages,
            lifecycle::get_page,
            lifecycle::get_page_by_id,
            lifecycle::list_projects,
            lifecycle::create_project,
            lifecycle::open_project,
            lifecycle::delete_project,
            lifecycle::close_project,
            lifecycle::import,
            lifecycle::select_page,
            editing::rename_page,
            editing::delete_pages,
            editing::move_page,
            editing::set_source_text,
            editing::set_translation,
            editing::set_text_content,
            editing::set_typography,
            editing::set_geometry,
            editing::set_visibility,
            editing::set_progress_markers,
            editing::delete_layers,
            editing::move_layer,
            editing::undo,
            editing::redo,
            processing::process,
            processing::stop_job,
            output::export,
            output::get_thumbnail,
            fonts::get_fonts,
            fonts::get_font_preview,
            preferences::save_preferences,
            preferences::get_preferences,
            preferences::get_translation_models,
            preferences::get_gemini_api_keys,
            preferences::set_gemini_api_key_manual_status,
            preferences::add_gemini_api_key,
            preferences::replace_gemini_api_key,
            preferences::remove_gemini_api_key,
            preferences::clear_gemini_api_keys,
            preferences::test_gemini_api_key,
            canvas::get_canvas_manifest,
            canvas::get_canvas_resource,
            canvas::prepare_canvas_page,
            canvas::preview_text_layout,
            canvas::get_canvas_page_manifest,
            canvas::get_canvas_page_resource,
            canvas::add_point_text,
            canvas::add_text_box,
            canvas::commit_paint,
            canvas::commit_erase,
            canvas::commit_transform,
            canvas::commit_inpaint,
        ])
        .disable_serde_phases()
        .error_handling(ErrorHandlingMode::Throw)
}
