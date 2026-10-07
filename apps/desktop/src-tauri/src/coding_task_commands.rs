//! Stage 5 coding-task Tauri surface (design §8.2): thin glue over owner A's
//! `pai_harness_adapter::coding_task::CodingTaskService`.
//!
//! Rules this file keeps (the source scans in `glue_policy::tests` check them):
//! * Every command is one delegation, `service.<method>(..)`, run through
//!   `tauri::async_runtime::spawn_blocking`. The ledger does vault I/O and
//!   gates/previews block for minutes, and Tauri v2 runs sync commands on the
//!   main thread. Errors become `String` through `ui_error` (A's messages
//!   carry no file content).
//! * Every command that changes task state, applies, resolves or exports first
//!   calls `main_only(&window)?`: it answers only the `main` webview, i.e. the
//!   UI event path. `coding_task_view` is callable anywhere, but the preview
//!   capability URL is stripped unless the caller is `main`.
//! * No model-role method (`record_plan`, `propose_edit`, `record_narrative`,
//!   `run_repair_loop`) is exposed, and nothing here is registered as a
//!   harness/model tool (`harness_bridge.rs` is untouched).
//! * Nothing here spawns a process, reads the host-command toggle or uses the
//!   host-command lane. On non-Linux the service itself answers
//!   `Unsupported` / `IsolationUnavailable` / `WorktreeUnavailable` before
//!   any port runs.
//! * `coding_task_open` admits `root` only when it is already inside a folder
//!   the user granted (Settings / in-chat grant). No grant is ever created.

mod glue_policy;

use pai_harness_adapter::coding_task::ports::{FileDiff, LogChunk, UiReviewEvent};
use pai_harness_adapter::coding_task::{
    ApplyReport, CodingTaskService, GateTarget, IsolationCapability, OpenTaskRequest,
    PlatformClass, PreviewView, ServiceConfig, TaskError, TaskSummary, TaskView, UiApplyEvent,
    UiPlanConfirmEvent, UiPreviewEvent, UiReconcileEvent, UiResumeEvent, UiRevertAppliedEvent,
    UiRevertEvent,
};
use pai_harness_adapter::task_ledger::TaskId;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use unoone_vault_core::Vault;

// Merged API: `CodingTaskService::new(vault, ServiceConfig)` wires the real
// production ports internally (no injectable fakes reachable from the glue).

/// Emitted after every successful state-changing command. The payload carries
/// no task content.
pub(crate) const UPDATED_EVENT: &str = "unoone:coding-task-updated";

#[derive(Clone, serde::Serialize)]
struct CodingTaskUpdated {
    task_id: String,
    /// `None` when the command returns something other than a `TaskView`.
    view_seq: Option<u64>,
}

/// Managed state. The service shares the single canonical vault `Arc` held
/// by `DesktopVaultState`.
pub(crate) struct CodingTaskState(pub Arc<CodingTaskService>);

impl CodingTaskState {
    /// Computes paths only. It never spawns, and the service probes the
    /// isolation capability lazily.
    pub(crate) fn new(vault: Arc<Mutex<Option<Vault>>>) -> Self {
        let scratch = glue_policy::scratch_dir(&std::env::temp_dir());
        // The service requires an existing canonical base. Creating the per-user
        // app-data directory is the only disk effect at startup; on failure the
        // base stays empty and the worktree policy denies apply (fail closed).
        let worktree_base =
            glue_policy::worktree_base(std::env::var_os("LOCALAPPDATA"), std::env::var_os("HOME"))
                .and_then(|base| {
                    std::fs::create_dir_all(&base).ok()?;
                    std::fs::canonicalize(&base).ok()
                })
                .unwrap_or_default();
        let config = ServiceConfig {
            scratch,
            worktree_base,
            worktree_deny_within: glue_policy::static_deny_within(std::env::current_exe().ok()),
            platform: PlatformClass::current(),
            // Ignored by the service by design; coding tasks never use the host lane.
            host_commands_enabled: false,
        };
        Self(Arc::new(CodingTaskService::new(vault, config)))
    }

    fn service(&self) -> Arc<CodingTaskService> {
        Arc::clone(&self.0)
    }

    /// Called from `stop_desktop_work` right after `emergency_lock()`, so
    /// admission is already closed when runs are cancelled and previews stop.
    pub(crate) fn on_lock(&self) {
        self.0.on_lock();
    }
}

fn ui_error(error: TaskError) -> String {
    error.to_string()
}

fn main_only(window: &tauri::WebviewWindow) -> Result<(), String> {
    glue_policy::require_main_window(window.label())
}

fn task_id_of(raw: &str) -> Result<TaskId, String> {
    TaskId::parse(raw).map_err(|error| ui_error(error.into()))
}

fn notify(app: &tauri::AppHandle, task_id: &str, view_seq: Option<u64>) {
    let _ = app.emit(
        UPDATED_EVENT,
        CodingTaskUpdated {
            task_id: task_id.to_owned(),
            view_seq,
        },
    );
}

/// Emit the content-free update event for a returned view, then hand it back.
fn published(app: &tauri::AppHandle, view: TaskView) -> TaskView {
    notify(app, &view.task_id, Some(view.view_seq));
    view
}

/// Run one service call off the async executor.
async fn blocking<T, F>(work: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, TaskError> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("coding task worker failed: {error}"))?
        .map_err(ui_error)
}

/// HA31: `root` must already be inside a granted folder (the same set the
/// harness file tools honour: effective workspace root plus every additional
/// granted folder). Returns the canonical root that was checked.
async fn granted_root(requested: &Path) -> Result<PathBuf, String> {
    let info = crate::harness_bridge::get_agent_workspace_info().await?;
    let granted = glue_policy::granted_roots(
        &info.effective_root,
        info.folders.iter().map(|folder| folder.root.as_str()),
    );
    glue_policy::admit_granted_root(requested, &granted)
}

#[tauri::command]
pub(crate) async fn coding_task_capability(
    state: tauri::State<'_, CodingTaskState>,
) -> Result<IsolationCapability, String> {
    let service = state.service();
    blocking(move || Ok(service.capability())).await
}

#[tauri::command]
pub(crate) async fn coding_task_list(
    state: tauri::State<'_, CodingTaskState>,
) -> Result<Vec<TaskSummary>, String> {
    let service = state.service();
    blocking(move || service.list_tasks()).await
}

#[tauri::command]
pub(crate) async fn coding_task_open(
    request: OpenTaskRequest,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let mut request = request;
    request.root = granted_root(&request.root).await?;
    let service = state.service();
    let view = blocking(move || service.open_task(request)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_view(
    task_id: TaskId,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    let service = state.service();
    let mut view = blocking(move || service.task_view(&task_id)).await?;
    if !glue_policy::is_main_window(window.label()) {
        view.preview.capability_url = None;
    }
    Ok(view)
}

#[tauri::command]
pub(crate) async fn coding_task_file_diff(
    task_id: TaskId,
    path: String,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<FileDiff, String> {
    let service = state.service();
    blocking(move || service.file_diff(&task_id, &path)).await
}

#[tauri::command]
pub(crate) async fn coding_task_confirm_plan(
    event: UiPlanConfirmEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let service = state.service();
    let view = blocking(move || service.confirm_plan(&task, event)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_run_gate(
    task_id: TaskId,
    target: GateTarget,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let service = state.service();
    let view = blocking(move || service.run_gate(&task_id, target)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_review_file(
    event: UiReviewEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let service = state.service();
    let view = blocking(move || service.review_file(&task, event)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_revert_file(
    event: UiRevertEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let service = state.service();
    let view = blocking(move || service.revert_file(&task, event)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_apply(
    event: UiApplyEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<ApplyReport, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let task_key = task.as_str().to_owned();
    let service = state.service();
    let report = blocking(move || service.apply(&task, event)).await?;
    notify(&app, &task_key, None);
    Ok(report)
}

#[tauri::command]
pub(crate) async fn coding_task_revert_applied(
    event: UiRevertAppliedEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<ApplyReport, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let task_key = task.as_str().to_owned();
    let service = state.service();
    let report = blocking(move || service.revert_applied(&task, event)).await?;
    notify(&app, &task_key, None);
    Ok(report)
}

#[tauri::command]
pub(crate) async fn coding_task_start_preview(
    event: UiPreviewEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<PreviewView, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let task_key = task.as_str().to_owned();
    let service = state.service();
    let preview = blocking(move || service.start_preview(&task, event)).await?;
    notify(&app, &task_key, None);
    Ok(preview)
}

#[tauri::command]
pub(crate) async fn coding_task_stop_preview(
    task_id: TaskId,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let service = state.service();
    let view = blocking(move || service.stop_preview(&task_id)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_preview_logs(
    task_id: TaskId,
    cursor: u64,
    limit: u32,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<LogChunk, String> {
    let service = state.service();
    blocking(move || service.preview_logs(&task_id, cursor, limit)).await
}

#[tauri::command]
pub(crate) async fn coding_task_http_checks(
    task_id: TaskId,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let service = state.service();
    let view = blocking(move || service.run_http_checks(&task_id)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_resolve_interrupted(
    event: UiReconcileEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let service = state.service();
    let view = blocking(move || service.resolve_interrupted(&task, event)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_resume(
    event: UiResumeEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<TaskView, String> {
    main_only(&window)?;
    let task = task_id_of(&event.task_id)?;
    let service = state.service();
    let view = blocking(move || service.resume(&task, event)).await?;
    Ok(published(&app, view))
}

#[tauri::command]
pub(crate) async fn coding_task_cancel(
    task_id: TaskId,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<(), String> {
    main_only(&window)?;
    let task_key = task_id.as_str().to_owned();
    let service = state.service();
    blocking(move || service.cancel(&task_id)).await?;
    notify(&app, &task_key, None);
    Ok(())
}

#[tauri::command]
pub(crate) async fn coding_task_export_patch(
    task_id: TaskId,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, CodingTaskState>,
) -> Result<String, String> {
    main_only(&window)?;
    let service = state.service();
    blocking(move || service.export_patch(&task_id)).await
}
