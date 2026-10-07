//! Stage 6 knowledge + coding-task learning Tauri surface (design §3.1): thin
//! glue over K1's `pai_harness_adapter::knowledge_service::KnowledgeService`
//! (with the K1 distiller) and K2's `task_learning::TaskLearning`.
//!
//! Rules this file keeps (the source scans in `glue_policy::tests` check them):
//! * Every command is one delegation (`service.<method>(..)`,
//!   `learning.<method>(..)`, or the pure `distill_request_sha256(..)` hash
//!   helper) run through `tauri::async_runtime::spawn_blocking`: the services
//!   do vault I/O and Stage 4 verification blocks, and Tauri v2 runs sync
//!   commands on the main thread. Errors become `String` through `ui_error`
//!   (the services' errors are fixed classifications without payload).
//! * Every command that changes knowledge or promotion state, approves,
//!   revokes or exports first calls `main_only(&window)?`: it answers only
//!   the `main` webview, i.e. the UI event path. Nothing here is registered
//!   as a harness/model tool (`harness_bridge.rs` is untouched).
//! * `DistillSource::LocalFile.root` is admitted only when it already sits
//!   inside a folder the user granted (the coding_task_open gate) and is
//!   replaced by its canonical path BEFORE the request is hashed or
//!   distilled, so the preview hash and the distill hash cover the same
//!   checked root. No grant is ever created.
//! * Nothing here spawns a process or writes a file. On non-Linux the
//!   services themselves answer `Unsupported` before any spawn.
//! * State-changing commands emit the content-free `unoone:knowledge-updated`.

mod glue_policy;

use pai_harness_adapter::coding_task::CodingTaskService;
use pai_harness_adapter::knowledge_distiller::{
    distill_request_sha256, DistillReport, DistillRequest, DistillRunSummary, DistillSource,
    UiDistillEvent,
};
use pai_harness_adapter::knowledge_service::{
    ExportBundle, ExportPreview, ExportRequest, KnowledgeDetailView, KnowledgeListFilter,
    KnowledgeListView, KnowledgeQuery, KnowledgeSearchView, KnowledgeService,
    KnowledgeServiceConfig, KnowledgeStatus, UiExportConsentEvent, UiInitEvent, UiRejectEvent,
    UiRevokeEvent,
};
use pai_harness_adapter::task_learning::{
    CandidateProposalView, LearningError, LearningVerificationView, RelevantPatternsView,
    TaskLearning, UiApprovePatternEvent, UiProposeCandidateEvent, UiRevokePatternEvent,
    UiVerifyCandidateEvent, VerificationPreview,
};
use pai_harness_adapter::task_ledger::TaskId;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use unoone_capability_contracts::knowledge::RecordRef;
use unoone_vault_core::Vault;

/// Emitted after every successful state-changing command. The payload carries
/// no knowledge content.
pub(crate) const UPDATED_EVENT: &str = "unoone:knowledge-updated";

#[derive(Clone, serde::Serialize)]
struct KnowledgeUpdated {
    command: &'static str,
    /// The coding task a learning command acted on; `None` for knowledge commands.
    task_id: Option<String>,
}

/// `knowledge_distill_preview` return shape (design §3.1 `{ request_sha256 }`).
#[derive(serde::Serialize)]
pub(crate) struct DistillPreview {
    request_sha256: String,
}

/// Managed state. Both services share the single canonical vault `Arc` held
/// by `DesktopVaultState`, and the learning loop shares the SAME
/// `Arc<CodingTaskService>` as `CodingTaskState`.
pub(crate) struct KnowledgeState {
    service: Arc<KnowledgeService>,
    learning: Arc<TaskLearning>,
}

impl KnowledgeState {
    /// Computes paths and reads the optional exclusions file (best-effort;
    /// missing = empty set). It never spawns and never writes.
    pub(crate) fn new(vault: Arc<Mutex<Option<Vault>>>, tasks: Arc<CodingTaskService>) -> Self {
        let temp = std::env::temp_dir();
        let exclusions = glue_policy::exclusions_file(
            std::env::var_os("LOCALAPPDATA"),
            std::env::var_os("HOME"),
        );
        let config = KnowledgeServiceConfig {
            scratch: glue_policy::scratch_dir(&temp),
            excluded_sha256: glue_policy::load_exclusions(exclusions.as_deref())
                .into_iter()
                .collect(),
            excluded_name_markers: glue_policy::excluded_name_markers(),
        };
        let service = Arc::new(KnowledgeService::new(Arc::clone(&vault), config));
        let learning = Arc::new(TaskLearning::new(
            vault,
            tasks,
            glue_policy::learning_scratch_dir(&temp),
        ));
        Self { service, learning }
    }

    fn service(&self) -> Arc<KnowledgeService> {
        Arc::clone(&self.service)
    }

    fn learning(&self) -> Arc<TaskLearning> {
        Arc::clone(&self.learning)
    }

    /// Called from `stop_desktop_work` right after `emergency_lock()` and the
    /// coding-task hook: aborts in-flight distillation before its next write,
    /// forgets used UI event ids, cancels in-flight verifications and drops
    /// the cached verifier.
    pub(crate) fn on_lock(&self) {
        self.service.on_lock();
        self.learning.on_lock();
    }
}

fn ui_error<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

fn main_only(window: &tauri::WebviewWindow) -> Result<(), String> {
    glue_policy::require_main_window(window.label())
}

fn task_id_of(raw: &str) -> Result<TaskId, String> {
    TaskId::parse(raw).map_err(|_| ui_error(LearningError::Invalid))
}

fn notify(app: &tauri::AppHandle, command: &'static str, task_id: Option<&TaskId>) {
    let _ = app.emit(
        UPDATED_EVENT,
        KnowledgeUpdated {
            command,
            task_id: task_id.map(|task| task.as_str().to_owned()),
        },
    );
}

/// Run one service call off the async executor.
async fn blocking<T, E, F>(work: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("knowledge worker failed: {error}"))?
        .map_err(ui_error)
}

/// The folders the user granted (effective workspace root plus every
/// additional granted folder), exactly as `coding_task_open` reads them.
async fn granted_folders() -> Result<Vec<PathBuf>, String> {
    let info = crate::harness_bridge::get_agent_workspace_info().await?;
    Ok(glue_policy::granted_roots(
        &info.effective_root,
        info.folders.iter().map(|folder| folder.root.as_str()),
    ))
}

/// Every `LocalFile` source: `root` must already be inside a granted folder
/// and is replaced by its canonical path; `path` must be a plain relative
/// path. Pasted text passes unchanged. Grants are read only when needed.
async fn admit_local_sources(request: DistillRequest) -> Result<DistillRequest, String> {
    let mut request = request;
    if !request
        .sources
        .iter()
        .any(|source| matches!(source, DistillSource::LocalFile { .. }))
    {
        return Ok(request);
    }
    let granted = granted_folders().await?;
    for source in request.sources.iter_mut() {
        if let DistillSource::LocalFile { root, path } = source {
            glue_policy::admit_relative_path(path)?;
            *root = glue_policy::admit_granted_root(root, &granted)?;
        }
    }
    Ok(request)
}

#[tauri::command]
pub(crate) async fn knowledge_status(
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeStatus, String> {
    let service = state.service();
    blocking(move || service.status()).await
}

#[tauri::command]
pub(crate) async fn knowledge_initialize(
    event: UiInitEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeStatus, String> {
    main_only(&window)?;
    let service = state.service();
    let status = blocking(move || service.initialize(event)).await?;
    notify(&app, "knowledge_initialize", None);
    Ok(status)
}

#[tauri::command]
pub(crate) async fn knowledge_rebuild_index(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeStatus, String> {
    main_only(&window)?;
    let service = state.service();
    let status = blocking(move || service.rebuild_index()).await?;
    notify(&app, "knowledge_rebuild_index", None);
    Ok(status)
}

#[tauri::command]
pub(crate) async fn knowledge_search(
    query: KnowledgeQuery,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeSearchView, String> {
    let service = state.service();
    blocking(move || service.search(query)).await
}

#[tauri::command]
pub(crate) async fn knowledge_list(
    filter: KnowledgeListFilter,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeListView, String> {
    let service = state.service();
    blocking(move || service.list(filter)).await
}

#[tauri::command]
pub(crate) async fn knowledge_detail(
    logical_id: String,
    revision: Option<u32>,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeDetailView, String> {
    let service = state.service();
    blocking(move || service.detail(&logical_id, revision)).await
}

#[tauri::command]
pub(crate) async fn knowledge_reject(
    event: UiRejectEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeDetailView, String> {
    main_only(&window)?;
    let service = state.service();
    let detail = blocking(move || service.reject(event)).await?;
    notify(&app, "knowledge_reject", None);
    Ok(detail)
}

#[tauri::command]
pub(crate) async fn knowledge_revoke_approval(
    event: UiRevokeEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<KnowledgeDetailView, String> {
    main_only(&window)?;
    let service = state.service();
    let detail = blocking(move || service.revoke_approval(event)).await?;
    notify(&app, "knowledge_revoke_approval", None);
    Ok(detail)
}

#[tauri::command]
pub(crate) async fn knowledge_export_preview(
    request: ExportRequest,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<ExportPreview, String> {
    let service = state.service();
    blocking(move || service.export_preview(request)).await
}

#[tauri::command]
pub(crate) async fn knowledge_export(
    event: UiExportConsentEvent,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<ExportBundle, String> {
    main_only(&window)?;
    let service = state.service();
    blocking(move || service.export(event)).await
}

/// Pure hash helper (no vault access, so no managed state): the UI shows this
/// hash with the plan and `knowledge_distill` must be confirmed with it.
#[tauri::command]
pub(crate) async fn knowledge_distill_preview(
    request: DistillRequest,
) -> Result<DistillPreview, String> {
    let request = admit_local_sources(request).await?;
    blocking(move || {
        distill_request_sha256(&request).map(|request_sha256| DistillPreview { request_sha256 })
    })
    .await
}

#[tauri::command]
pub(crate) async fn knowledge_distill(
    request: DistillRequest,
    event: UiDistillEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<DistillReport, String> {
    main_only(&window)?;
    let request = admit_local_sources(request).await?;
    let service = state.service();
    let report = blocking(move || service.distill(request, event)).await?;
    notify(&app, "knowledge_distill", None);
    Ok(report)
}

#[tauri::command]
pub(crate) async fn knowledge_distill_runs(
    state: tauri::State<'_, KnowledgeState>,
) -> Result<Vec<DistillRunSummary>, String> {
    let service = state.service();
    blocking(move || service.distill_runs()).await
}

#[tauri::command]
pub(crate) async fn task_relevant_patterns(
    task_id: String,
    limit: usize,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<RelevantPatternsView, String> {
    let task = task_id_of(&task_id)?;
    let learning = state.learning();
    blocking(move || learning.relevant_patterns(&task, limit)).await
}

#[tauri::command]
pub(crate) async fn task_propose_candidate(
    task_id: String,
    event: UiProposeCandidateEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<CandidateProposalView, String> {
    main_only(&window)?;
    let task = task_id_of(&task_id)?;
    let task_key = task.clone();
    let learning = state.learning();
    let proposal = blocking(move || learning.propose_candidate(&task, event)).await?;
    notify(&app, "task_propose_candidate", Some(&task_key));
    Ok(proposal)
}

#[tauri::command]
pub(crate) async fn task_verification_preview(
    task_id: String,
    candidate: RecordRef,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<VerificationPreview, String> {
    let task = task_id_of(&task_id)?;
    let learning = state.learning();
    blocking(move || learning.verification_preview(&task, &candidate)).await
}

#[tauri::command]
pub(crate) async fn task_verify_candidate(
    task_id: String,
    event: UiVerifyCandidateEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<LearningVerificationView, String> {
    main_only(&window)?;
    let task = task_id_of(&task_id)?;
    let task_key = task.clone();
    let learning = state.learning();
    let view = blocking(move || learning.verify_candidate(&task, event)).await?;
    notify(&app, "task_verify_candidate", Some(&task_key));
    Ok(view)
}

#[tauri::command]
pub(crate) async fn task_approve_pattern(
    task_id: String,
    event: UiApprovePatternEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<LearningVerificationView, String> {
    main_only(&window)?;
    let task = task_id_of(&task_id)?;
    let task_key = task.clone();
    let learning = state.learning();
    let view = blocking(move || learning.approve_pattern(&task, event)).await?;
    notify(&app, "task_approve_pattern", Some(&task_key));
    Ok(view)
}

#[tauri::command]
pub(crate) async fn task_revoke_pattern(
    task_id: String,
    event: UiRevokePatternEvent,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, KnowledgeState>,
) -> Result<LearningVerificationView, String> {
    main_only(&window)?;
    let task = task_id_of(&task_id)?;
    let task_key = task.clone();
    let learning = state.learning();
    let view = blocking(move || learning.revoke_pattern(&task, event)).await?;
    notify(&app, "task_revoke_pattern", Some(&task_key));
    Ok(view)
}
