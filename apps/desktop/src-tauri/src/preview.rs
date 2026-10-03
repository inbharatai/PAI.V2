//! Live website preview (2026-10-02 universal-agent cycle): the agent
//! builds a site with `fs.write` and the user watches it render in its own
//! window — with NO web server anywhere on the machine.
//!
//! **How it works.** `web.preview` hands this module the entry `.html`
//! inside a granted folder. The site's containing folder is MIRROLED into
//! [`PreviewState::mirror_root`] — `%TEMP%\unoone-preview` in the app
//! (bounded: ≤ [`MAX_FILES`] files, ≤ [`MAX_TOTAL_BYTES`] bytes, ≤
//! [`MAX_DEPTH`] nesting, build dirs skipped), which is the only tree the
//! asset protocol gains scope for. The
//! frontend then creates the preview window on the mirror's entry file
//! through the proven JS path — a window built from Rust never starts its
//! WebView2 content process (defects #40/#41, live-caught 2026-09-15), so
//! the backend only EMITS `unoone:ensure-preview-window` with the staged
//! path; `App.tsx` listens and opens it.
//!
//! **Reload-on-write.** The frontend heartbeats [`preview_poll`] while a
//! session is active. The poll walks the source tree, hashes
//! (path, size, mtime) of every file, and only when that signature changes
//! re-stages the mirror and evals `location.reload()` in the preview
//! window (backend evals are not capability-gated — the proven
//! `browser.rs` pattern). No file-watcher dependency, no port, no server.
//!
//! **Trust boundary.** The preview window is created with NO Tauri
//! capabilities (`capabilities/default.json` scopes every permission to the
//! `main` window), so scripts inside the previewed page cannot touch the
//! IPC surface — the page is data, not an extension of the app. The mirror
//! is a copy of user-authored content from a granted folder, bounded by
//! the walk limits above, and symlinks/junctions never mirror so the
//! mirror contains exactly what the bounded walk saw.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;
use tauri::{Emitter, Manager};

/// The preview window's label — created by the frontend (defects #40/#41:
/// Rust-created window shells never start their WebView2 content process).
pub(crate) const PREVIEW_WINDOW_LABEL: &str = "agent-preview";

/// The mirror tree under `$TEMP` — the default mirror root and the only
/// path the asset protocol gains scope for (tauri.conf.json
/// `assetProtocol.scope`).
const MIRROR_DIR_NAME: &str = "unoone-preview";

/// One mirrored site: at most 128 files…
const MAX_FILES: usize = 128;
/// …at most 16 MiB total…
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
/// …nested at most 8 directories deep.
const MAX_DEPTH: usize = 8;

/// Build-tool output that is never part of the site the user watches.
/// Compared ASCII-case-insensitively (Windows filesystems).
const SKIP_DIRS: [&str; 4] = ["node_modules", ".git", "target", ".next"];

/// The mirror directory. `$TEMP` resolves per-user; the asset protocol
/// scope spells it the same way.
pub(crate) fn preview_dir() -> PathBuf {
    std::env::temp_dir().join(MIRROR_DIR_NAME)
}

/// One file of the walked site, relative to the site root.
struct SourceFile {
    rel: PathBuf,
    mtime: SystemTime,
    len: u64,
}

/// The bounded walk of one site folder: collects every file and a content
/// signature (hash of path + size + mtime per file) that changes whenever
/// any file is added, removed, rewritten or resized. Symlinks and junctions
/// are skipped so the mirror can never contain anything the walk did not
/// see inside the granted folder.
fn walk_source(root: &Path) -> Result<(Vec<SourceFile>, u64, u64), String> {
    let mut files: Vec<SourceFile> = Vec::new();
    let mut total_bytes: u64 = 0;
    walk_dir(root, Path::new(""), 0, &mut files, &mut total_bytes)?;
    let mut hasher = DefaultHasher::new();
    for file in &files {
        file.rel.hash(&mut hasher);
        file.len.hash(&mut hasher);
        file.mtime.hash(&mut hasher);
    }
    Ok((files, total_bytes, hasher.finish()))
}

fn walk_dir(
    root: &Path,
    rel: &Path,
    depth: usize,
    files: &mut Vec<SourceFile>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "the site is nested deeper than {MAX_DEPTH} directories — too deep to preview"
        ));
    }
    let dir = root.join(rel);
    let entries = std::fs::read_dir(&dir)
        .map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot scan {}: {error}", dir.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot classify {}: {error}", dir.display()))?;
        // Symlinks/junctions never mirror — the mirror must contain exactly
        // what the walk saw, inside the granted folder.
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let child_rel = rel.join(&name);
        if file_type.is_dir() {
            if SKIP_DIRS.iter().any(|skip| name.eq_ignore_ascii_case(skip)) {
                continue;
            }
            walk_dir(root, &child_rel, depth + 1, files, total_bytes)?;
        } else if file_type.is_file() {
            if files.len() >= MAX_FILES {
                return Err(format!(
                    "the site has more than {MAX_FILES} files — too large to preview"
                ));
            }
            let metadata = entry
                .metadata()
                .map_err(|error| format!("cannot stat {}: {error}", child_rel.display()))?;
            *total_bytes += metadata.len();
            if *total_bytes > MAX_TOTAL_BYTES {
                return Err(format!(
                    "the site exceeds {} MiB — too large to preview",
                    MAX_TOTAL_BYTES / (1024 * 1024)
                ));
            }
            files.push(SourceFile {
                rel: child_rel,
                mtime: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                len: metadata.len(),
            });
        }
    }
    Ok(())
}

/// Stages the walked files into the mirror root: the previous mirror is
/// removed, then every file is copied. The only callers re-stage BEFORE the
/// preview window reloads (the tool before opening it, the poll before
/// eval-ing reload), so the window never reads a half-written mirror. The
/// root's path is stable for the whole session — that is what lets the
/// reload be a plain `location.reload()` instead of a retarget.
fn stage_mirror(root: &Path, files: &[SourceFile], mirror: &Path) -> Result<(), String> {
    if mirror.exists() {
        std::fs::remove_dir_all(mirror)
            .map_err(|error| format!("cannot clear the old preview mirror: {error}"))?;
    }
    std::fs::create_dir_all(mirror)
        .map_err(|error| format!("cannot create the preview mirror: {error}"))?;
    for file in files {
        let source = root.join(&file.rel);
        let dest = mirror.join(&file.rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        std::fs::copy(&source, &dest)
            .map_err(|error| format!("cannot mirror {}: {error}", file.rel.display()))?;
    }
    Ok(())
}

/// The active preview: which granted-folder site is being watched.
pub(crate) struct PreviewSession {
    /// The site's containing folder (the entry's parent, inside a grant).
    root: PathBuf,
    /// The mirror root this session stages into (from
    /// [`PreviewState::mirror_root`]; the path is stable for the session's
    /// lifetime so reloads stay `location.reload()`).
    mirror_root: PathBuf,
    /// The entry `.html` file name — the same name inside root and mirror.
    entry_name: PathBuf,
    /// The signature the mirror was last staged from.
    signature: u64,
}

impl PreviewSession {
    /// The entry file inside the MIRROR — what the frontend turns into an
    /// asset URL and what `preview_focus` re-opens.
    fn mirror_entry(&self) -> PathBuf {
        self.mirror_root.join(&self.entry_name)
    }
}

/// Managed Tauri state: at most one live preview session.
pub(crate) struct PreviewState {
    session: Mutex<Option<PreviewSession>>,
    blocked: std::sync::atomic::AtomicBool,
    /// Where this state stages its mirrors — `%TEMP%\unoone-preview` in
    /// the app; tests point it at an isolated directory so parallel test
    /// stagings can never delete each other's mirrors.
    mirror_root: PathBuf,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            session: Mutex::new(None),
            blocked: std::sync::atomic::AtomicBool::new(false),
            mirror_root: preview_dir(),
        }
    }
}

impl PreviewState {
    pub(crate) fn resume(&self) {
        let _guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        self.blocked
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// A state whose mirrors stage under `mirror_root` (tests use this for
    /// isolation; the app uses [`PreviewState::default`]).
    #[cfg(test)]
    pub(crate) fn with_mirror_root(mirror_root: PathBuf) -> Self {
        Self {
            session: Mutex::new(None),
            blocked: std::sync::atomic::AtomicBool::new(false),
            mirror_root,
        }
    }
}

/// What [`start_preview`] staged, for the tool's honest summary.
#[derive(Debug)]
pub(crate) struct PreviewSessionInfo {
    pub mirror_entry: PathBuf,
    pub file_count: usize,
    pub total_bytes: u64,
}

/// The event payload that asks the frontend to open (or retarget) the
/// preview window on the staged mirror entry.
#[derive(Clone, serde::Serialize)]
pub(crate) struct EnsurePreviewPayload {
    pub path: String,
}

/// Stages a site and records the session. `entry` is the caller's
/// RESOLVED absolute entry path (already fenced by `GrantedFolders`).
/// Only `.html`/`.htm` entries preview — the window renders documents.
pub(crate) fn start_preview(
    state: &PreviewState,
    entry: &Path,
) -> Result<PreviewSessionInfo, String> {
    let mut session = state.session.lock().map_err(|_| "Preview lock failed")?;
    if state.blocked.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("Unlock Pocket AI before starting a preview".to_owned());
    }
    let extension = entry
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "html" | "htm") {
        return Err(format!(
            "web.preview needs an .html entry page, not {}",
            entry.display()
        ));
    }
    let entry_name = entry
        .file_name()
        .ok_or_else(|| "the entry page has no file name".to_owned())?
        .to_owned();
    let root = entry
        .parent()
        .ok_or_else(|| "the entry page has no containing folder".to_owned())?
        .to_path_buf();
    let (files, total_bytes, signature) = walk_source(&root)?;
    if files.is_empty() {
        return Err("the site folder is empty — nothing to preview".to_owned());
    }
    stage_mirror(&root, &files, &state.mirror_root)?;
    let mirror_root = state.mirror_root.clone();
    *session = Some(PreviewSession {
        root,
        mirror_root,
        entry_name: entry_name.into(),
        signature,
    });
    Ok(PreviewSessionInfo {
        // The session just stored holds the entry name; read it back so the
        // two can never drift.
        mirror_entry: session.as_ref().expect("just stored").mirror_entry(),
        file_count: files.len(),
        total_bytes,
    })
}

/// One heartbeat tick of [`preview_poll`], standalone so it is testable
/// without a Tauri AppHandle.
enum PollOutcome {
    /// No session (or the site folder vanished): the frontend stops the
    /// heartbeat.
    Inactive,
    /// The signature matches the last staging: nothing to do.
    Unchanged,
    /// The site changed: the caller re-staged the mirror and must reload
    /// the preview window.
    Changed,
}

/// Checks the site for changes and re-stages the mirror when it did. The
/// session ends honestly when the site folder no longer exists.
fn poll_once(session: &mut Option<PreviewSession>) -> Result<PollOutcome, String> {
    let Some(current) = session.as_mut() else {
        return Ok(PollOutcome::Inactive);
    };
    if !current.root.is_dir() {
        // The site folder was deleted out from under the preview: end the
        // session and clear the mirror rather than serving a stale copy.
        let mirror_root = current.mirror_root.clone();
        *session = None;
        let _ = std::fs::remove_dir_all(&mirror_root);
        return Ok(PollOutcome::Inactive);
    }
    let (files, _total_bytes, signature) = walk_source(&current.root)?;
    if signature == current.signature {
        return Ok(PollOutcome::Unchanged);
    }
    stage_mirror(&current.root, &files, &current.mirror_root)?;
    current.signature = signature;
    Ok(PollOutcome::Changed)
}

/// Frontend heartbeat (~1.5s while a preview session is active): re-stages
/// the mirror when the site's files changed and reloads the preview window.
#[tauri::command]
pub(crate) fn preview_poll(
    state: tauri::State<PreviewState>,
    app: tauri::AppHandle,
) -> Result<PreviewPollResult, String> {
    let mut session = state.session.lock().expect("preview session lock");
    match poll_once(&mut session)? {
        PollOutcome::Inactive => Ok(PreviewPollResult { active: false }),
        PollOutcome::Unchanged => Ok(PreviewPollResult { active: true }),
        PollOutcome::Changed => {
            if let Some(window) = app.get_webview_window(PREVIEW_WINDOW_LABEL) {
                // The mirror is already re-staged above; the reload cannot
                // read a half-written tree.
                let _ = window.eval("window.location.reload()");
            }
            Ok(PreviewPollResult { active: true })
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct PreviewPollResult {
    pub active: bool,
}

/// Ends the preview session: closes the window and clears the mirror.
/// Returns whether a session was actually active.
#[tauri::command]
pub(crate) fn preview_stop(
    state: tauri::State<PreviewState>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    Ok(stop_session(&state, &app, false))
}

pub(crate) fn emergency_stop(app: &tauri::AppHandle) {
    stop_session(&app.state::<PreviewState>(), app, true);
}

fn stop_session(state: &PreviewState, app: &tauri::AppHandle, block: bool) -> bool {
    let mut session = state.session.lock().unwrap_or_else(|e| e.into_inner());
    if block {
        state
            .blocked
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let had = session.take().is_some();
    if let Some(window) = app.get_webview_window(PREVIEW_WINDOW_LABEL) {
        let _ = window.close();
    }
    let _ = std::fs::remove_dir_all(&state.mirror_root);
    had
}

/// The chat affordance: focuses the preview window, or re-opens it on the
/// current session when the user closed it. Returns false when no preview
/// session exists.
#[tauri::command]
pub(crate) fn preview_focus(
    state: tauri::State<PreviewState>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    if let Some(window) = app.get_webview_window(PREVIEW_WINDOW_LABEL) {
        let _ = window.set_focus();
        return Ok(true);
    }
    let session = state.session.lock().expect("preview session lock");
    match session.as_ref() {
        Some(current) => {
            app.emit(
                "unoone:ensure-preview-window",
                EnsurePreviewPayload {
                    path: current.mirror_entry().to_string_lossy().to_string(),
                },
            )
            .map_err(|error| format!("could not request the preview window: {error}"))?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// An isolated mirror root per test: the default `$TEMP\unoone-preview`
    /// is SHARED, and `cargo test` runs these in parallel — one test's
    /// staging (which clears the root) would delete another test's mirror
    /// mid-copy. The app only ever stages one session, but the tests must
    /// never race each other.
    fn mirror_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "unoone-preview-mirror-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    /// A small site: index.html + style.css + assets/app.js, plus a
    /// node_modules dir and a .git dir that must never mirror.
    fn site() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "unoone-preview-site-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(root.join("assets")).expect("site dirs");
        fs::create_dir_all(root.join("node_modules/left-pad")).expect("node_modules");
        fs::create_dir_all(root.join(".git")).expect("git dir");
        fs::write(root.join("index.html"), "<h1>one</h1>").expect("index");
        fs::write(root.join("style.css"), "h1 { color: red }").expect("css");
        fs::write(root.join("assets/app.js"), "console.log(1);").expect("js");
        fs::write(root.join("node_modules/left-pad/ignore.txt"), "junk").expect("junk");
        fs::write(root.join(".git/config"), "junk").expect("git junk");
        root
    }

    #[test]
    fn locked_preview_cannot_restage_until_unlock() {
        let root = site();
        let mirror = mirror_root();
        let state = PreviewState::with_mirror_root(mirror.clone());
        state
            .blocked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(start_preview(&state, &root.join("index.html")).is_err());
        assert!(!mirror.exists());
        state.resume();
        assert!(start_preview(&state, &root.join("index.html")).is_ok());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(mirror);
    }

    #[test]
    fn start_preview_mirrors_the_site_and_stores_the_session() {
        let root = site();
        let mirror = mirror_root();
        let state = PreviewState::with_mirror_root(mirror.clone());
        let info = start_preview(&state, &root.join("index.html")).expect("start preview");
        // The walked site is exactly the four real files; build dirs skip.
        assert_eq!(info.file_count, 3, "index.html + style.css + assets/app.js");
        assert!(info.mirror_entry.ends_with("index.html"));
        assert_eq!(
            fs::read_to_string(&info.mirror_entry).expect("mirror read"),
            "<h1>one</h1>"
        );
        assert!(mirror.join("assets/app.js").is_file());
        assert!(
            !mirror.join("node_modules").exists(),
            "node_modules never mirrors"
        );
        assert!(!mirror.join(".git").exists(), ".git never mirrors");
        // A non-entry file is refused.
        let error =
            start_preview(&state, &root.join("style.css")).expect_err("css is not an entry page");
        assert!(
            error.contains(".html"),
            "refusal names the entry rule: {error}"
        );
        fs::remove_dir_all(&root).ok();
        let _ = fs::remove_dir_all(&mirror);
    }

    #[test]
    fn poll_once_reloads_only_when_the_site_changes() {
        let root = site();
        let mirror = mirror_root();
        let state = PreviewState::with_mirror_root(mirror.clone());
        let mut session = {
            start_preview(&state, &root.join("index.html")).expect("start");
            let taken = std::mem::take(&mut *state.session.lock().expect("lock"));
            taken
        };
        assert!(matches!(
            poll_once(&mut session),
            Ok(PollOutcome::Unchanged)
        ));
        // Same-length rewrite can land in the same filesystem timestamp
        // quantum on a coarse clock, so give mtime room to move.
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(root.join("index.html"), "<h1>two</h1>").expect("edit");
        assert!(matches!(poll_once(&mut session), Ok(PollOutcome::Changed)));
        assert_eq!(
            fs::read_to_string(session.as_ref().expect("session").mirror_entry()).expect("mirror"),
            "<h1>two</h1>",
            "the re-staged mirror carries the edit"
        );
        assert!(matches!(
            poll_once(&mut session),
            Ok(PollOutcome::Unchanged)
        ));
        // A brand-new file also changes the signature.
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(root.join("about.html"), "<p>about</p>").expect("new page");
        assert!(matches!(poll_once(&mut session), Ok(PollOutcome::Changed)));
        assert!(mirror.join("about.html").is_file());
        fs::remove_dir_all(&root).ok();
        let _ = fs::remove_dir_all(&mirror);
    }

    #[test]
    fn poll_once_ends_the_session_when_the_site_folder_vanishes() {
        let root = site();
        let mirror = mirror_root();
        let state = PreviewState::with_mirror_root(mirror.clone());
        let mut session = {
            start_preview(&state, &root.join("index.html")).expect("start");
            let taken = std::mem::take(&mut *state.session.lock().expect("lock"));
            taken
        };
        fs::remove_dir_all(&root).expect("delete site");
        assert!(matches!(poll_once(&mut session), Ok(PollOutcome::Inactive)));
        assert!(session.is_none(), "the session is cleared");
        assert!(!mirror.exists(), "the stale mirror is removed");
    }

    #[test]
    fn the_walk_refuses_sites_beyond_the_bounds() {
        // 129 files: over MAX_FILES.
        let root = site();
        for index in 0..130 {
            fs::write(root.join(format!("page-{index}.html")), "x").expect("fill");
        }
        let error = start_preview(
            &PreviewState::with_mirror_root(mirror_root()),
            &root.join("index.html"),
        )
        .expect_err("too many files");
        assert!(error.contains("128"), "names the bound: {error}");
        fs::remove_dir_all(&root).ok();

        // Deeper than MAX_DEPTH.
        let root = std::env::temp_dir().join(format!(
            "unoone-preview-deep-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut deep = root.clone();
        for _ in 0..12 {
            deep = deep.join("level");
        }
        fs::create_dir_all(&deep).expect("deep dirs");
        fs::write(deep.join("leaf.txt"), "x").expect("leaf");
        fs::write(root.join("index.html"), "<p>deep</p>").expect("entry");
        let error = start_preview(
            &PreviewState::with_mirror_root(mirror_root()),
            &root.join("index.html"),
        )
        .expect_err("too deep");
        assert!(error.contains("8"), "names the bound: {error}");
        fs::remove_dir_all(&root).ok();

        // Over MAX_TOTAL_BYTES: a sparse 17 MiB file (set_len — instant).
        let root = std::env::temp_dir().join(format!(
            "unoone-preview-big-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).expect("big dir");
        fs::write(root.join("index.html"), "<p>big</p>").expect("entry");
        let big = root.join("blob.bin");
        fs::File::create(&big)
            .and_then(|file| file.set_len(MAX_TOTAL_BYTES + 1))
            .expect("sparse file");
        let error = start_preview(
            &PreviewState::with_mirror_root(mirror_root()),
            &root.join("index.html"),
        )
        .expect_err("too big");
        assert!(error.contains("16 MiB"), "names the bound: {error}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_site_folder_is_refused() {
        let root = std::env::temp_dir().join(format!(
            "unoone-preview-empty-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).expect("empty dir");
        let error = start_preview(
            &PreviewState::with_mirror_root(mirror_root()),
            &root.join("index.html"),
        )
        .expect_err("empty site");
        assert!(error.contains("empty"), "honest refusal: {error}");
        fs::remove_dir_all(&root).ok();
    }
}
