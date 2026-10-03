//! Multi-root file fencing for the desktop agent lane (P7 folder grants).
//!
//! The workspace grant (`agent-workspace.json`) fences the agent's file tools
//! to ONE user-approved folder. The P7 directive is broader: the user can
//! grant additional folders (e.g. the Desktop, a folder on the USB drive's
//! host sibling) so the agent can create real artifacts where the user
//! wants them — still only where a human said yes.
//!
//! [`GrantedFolders`] is a SET of `RootedFs` fences, one per granted folder,
//! with the workspace root as the primary (index 0). Every file operation is
//! ROUTED: relative paths resolve against the workspace root exactly as
//! before; absolute paths are matched against the granted roots by the
//! longest boundary-aware prefix, then delegated to the containing root's
//! own `RootedFs` — which re-runs its complete fence (lexical join, escape
//! scan, canonicalize, containment, TOCTOU checks, byte limits). Routing is
//! dispatch-only and fails closed: a routing bug can mis-select WHICH
//! granted folder handles an operation or deny it, never escape the union
//! of granted folders, because the selected `RootedFs` re-validates the
//! path against its own root before touching the filesystem.
//!
//! The prefix matcher mirrors the vendored `RootedFs::strip_root_prefix`
//! (vendor/inbharat-harness/crates/core/src/execution.rs) — verbatim `\\?\`
//! forms, case variants, `/`-vs-`\` separators and the component-boundary
//! guard (`C:\rootx` is NOT inside `C:\root`) — implemented here against the
//! public `root()` accessor so the security-adjacent logic lives in the
//! desktop crate, where `cargo test --workspace` (the CI Rust gate) actually
//! runs its unit tests; the vendored tree's own tests are excluded from the
//! outer workspace and never run in CI.
//!
//! [`GrantedFolderBroker`] is the `ExecutionBroker` the harness tools call:
//! file operations delegate to [`GrantedFolders`]; process operations
//! use a session-only host-command lease and retain process ownership for
//! lock/unplug cleanup. Folder grants do not authorize or sandbox programs.

use inbharat_harness_core::execution::{DetachedSpawn, ProcessOutput};
use inbharat_harness_core::{
    ExecutionBroker, Failure, FailureClass, HarnessResult, ProcessSpec, RootedFs,
};
use std::path::{Path, PathBuf};

/// The outcome of the in-chat folder-grant approval (2026-10-03): when a
/// routed absolute path lands outside every granted folder, the desktop
/// approval layer (see `request_folder_grant` in harness_bridge.rs) asks
/// the human with a chat-approval card and answers with one of these.
#[derive(Clone, Debug)]
pub(crate) enum DeniedPathResolution {
    /// The human granted a folder containing the path: the routed
    /// (fence, root-relative remainder) pair for THIS operation, built
    /// from the current persisted grants.
    Granted(RootedFs, PathBuf),
    /// The human declined, never answered in time, or the proposal failed:
    /// the reason becomes part of the failure the model sees.
    Denied(String),
}

/// The approval hook stored inside a [`GrantedFolders`] set. Blocking is
/// allowed: callers run on harness tool threads (`spawn_blocking`), and the
/// approval layer bounds the wait itself (deny by default).
pub(crate) type DeniedPathRequest =
    std::sync::Arc<dyn Fn(&Path) -> DeniedPathResolution + Send + Sync>;

/// The multi-root fence: one `RootedFs` per granted folder, workspace first.
#[derive(Clone)]
pub(crate) struct GrantedFolders {
    /// Index 0 is the workspace root (the primary); grants keep their
    /// persisted order after it.
    roots: Vec<RootedFs>,
    /// The in-chat approval hook: a path outside every grant asks the human
    /// (bounded, deny by default) instead of failing outright. `None` —
    /// tests, and any consumer that wants denials to stand — answers
    /// denials itself.
    denied_request: Option<DeniedPathRequest>,
}

impl std::fmt::Debug for GrantedFolders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantedFolders")
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

impl GrantedFolders {
    /// Builds the set from granted roots. The first root is the primary
    /// (workspace); at least one root is required because relative paths
    /// must have a deterministic anchor.
    pub(crate) fn new(roots: Vec<RootedFs>) -> HarnessResult<Self> {
        if roots.is_empty() {
            return Err(Failure::new(
                inbharat_harness_core::ErrorCode::FilesystemDenied,
                FailureClass::Policy,
                "fs.granted_folders",
                "the granted-folder set is empty — the workspace root is always required",
            ));
        }
        Ok(Self {
            roots,
            denied_request: None,
        })
    }

    /// Installs the in-chat approval hook (chainable builder).
    pub(crate) fn with_denied_request(mut self, hook: DeniedPathRequest) -> Self {
        self.denied_request = Some(hook);
        self
    }

    /// The workspace root: the anchor for every relative path and the
    /// working directory of process runs.
    pub(crate) fn primary(&self) -> &RootedFs {
        &self.roots[0]
    }

    /// Routes a caller-supplied path to the fence that contains it.
    /// Relative paths resolve against the workspace root; absolute paths
    /// match the longest boundary-aware root prefix. The returned path is
    /// ROOT-RELATIVE so the selected `RootedFs` re-fences it from scratch.
    ///
    /// An absolute path no grant covers asks the human (the approval hook,
    /// when installed): mid-run grants — the hook's own, a Settings grant,
    /// or another tool's — all resolve through the same re-check inside the
    /// approval layer without re-asking.
    fn route(&self, path: &Path) -> HarnessResult<(RootedFs, PathBuf)> {
        match self.route_once(path) {
            Ok((fenced, remainder)) => Ok((fenced.clone(), remainder)),
            Err(failure) => {
                // Only an ABSOLUTE routing failure can be widened by a
                // grant; a relative path escaping its root is a containment
                // refusal, not a missing grant, and must stand.
                if !path.is_absolute() {
                    return Err(failure);
                }
                let Some(request) = self.denied_request.as_ref() else {
                    return Err(failure);
                };
                match request(path) {
                    DeniedPathResolution::Granted(fenced, remainder) => Ok((fenced, remainder)),
                    DeniedPathResolution::Denied(reason) => {
                        Err(self.outside_failure(path, &reason))
                    }
                }
            }
        }
    }

    /// The grant-independent routing step: longest boundary-aware prefix
    /// match, no approval hook. The approval layer re-uses this against a
    /// set rebuilt from the CURRENT persisted grants (it must not recurse
    /// through its own hook).
    fn route_once(&self, path: &Path) -> HarnessResult<(&RootedFs, PathBuf)> {
        if !path.is_absolute() {
            return Ok((&self.roots[0], path.to_path_buf()));
        }
        // Longest root wins so a nested grant (e.g. both `D:\work` and
        // `D:\work\deep` granted) resolves to the innermost fence — the
        // tighter limits and honest error messages of the specific folder.
        let mut best: Option<(usize, &RootedFs, PathBuf)> = None;
        for fenced in &self.roots {
            let Some(remainder) = strip_root_prefix(fenced.root(), path) else {
                continue;
            };
            let length = fenced.root().as_os_str().len();
            let improves = match &best {
                Some((known, _, _)) => *known < length,
                None => true,
            };
            if improves {
                best = Some((length, fenced, remainder));
            }
        }
        best.map(|(_, fenced, remainder)| (fenced, remainder))
            .ok_or_else(|| self.outside_failure(path, ""))
    }

    /// Routes an absolute path WITHOUT the approval hook — the fresh-store
    /// re-check inside the approval layer itself.
    pub(crate) fn try_route_absolute(&self, path: &Path) -> Option<(RootedFs, PathBuf)> {
        if !path.is_absolute() {
            return None;
        }
        self.route_once(path)
            .ok()
            .map(|(fenced, remainder)| (fenced.clone(), remainder))
    }

    /// The honest denial for an absolute path no grant covers: names the
    /// path (the message is all the model sees) and, in the details, every
    /// granted root so the UI can show the current scope.
    fn outside_failure(&self, path: &Path, reason: &str) -> Failure {
        let granted = self
            .roots
            .iter()
            .map(|fenced| fenced.root().display().to_string())
            .collect::<Vec<_>>()
            .join("; ");
        let message = if reason.is_empty() {
            format!(
                "the path '{}' is outside every folder the user granted the agent",
                path.display()
            )
        } else {
            format!(
                "the path '{}' is outside every folder the user granted the agent — {reason}",
                path.display()
            )
        };
        Failure::new(
            inbharat_harness_core::ErrorCode::FilesystemDenied,
            FailureClass::Policy,
            "fs.route",
            message,
        )
        .with_detail("path", path.display().to_string())
        .with_detail("granted_folders", granted)
    }

    /// Reads a bounded UTF-8 file inside any granted folder.
    pub(crate) fn read_text(&self, path: impl AsRef<Path>) -> HarnessResult<String> {
        let (fenced, remainder) = self.route(path.as_ref())?;
        fenced.read_text(remainder)
    }

    /// Lists one directory inside any granted folder.
    pub(crate) fn list(&self, path: impl AsRef<Path>) -> HarnessResult<Vec<String>> {
        let (fenced, remainder) = self.route(path.as_ref())?;
        fenced.list(remainder)
    }

    /// Atomically writes text inside any granted folder.
    pub(crate) fn write_text_atomic(
        &self,
        path: impl AsRef<Path>,
        contents: &str,
    ) -> HarnessResult<()> {
        let (fenced, remainder) = self.route(path.as_ref())?;
        fenced.write_text_atomic(remainder, contents)
    }

    /// Atomically writes binary contents inside any granted folder — the
    /// document-creation lane (real PDF/DOCX files), same fence as text.
    pub(crate) fn write_bytes_atomic(
        &self,
        path: impl AsRef<Path>,
        contents: &[u8],
    ) -> HarnessResult<()> {
        let (fenced, remainder) = self.route(path.as_ref())?;
        fenced.write_bytes_atomic(remainder, contents)
    }

    /// Creates a directory tree inside any granted folder.
    pub(crate) fn create_dir_all(&self, path: impl AsRef<Path>) -> HarnessResult<()> {
        let (fenced, remainder) = self.route(path.as_ref())?;
        fenced.create_dir_all(remainder)
    }

    /// Resolves an existing path (canonical, containment-checked) inside
    /// any granted folder.
    pub(crate) fn resolve_existing(&self, path: impl AsRef<Path>) -> HarnessResult<PathBuf> {
        let (fenced, remainder) = self.route(path.as_ref())?;
        fenced.resolve_existing(remainder)
    }

    /// Binary-safe copy between any two granted folders (design-website
    /// lane, 2026-10-03). `fs.read` is UTF-8-only, so an agent "copying" an
    /// image through read+write corrupts it — this path never touches a
    /// text lane. Both ends are routed independently, so a missing grant
    /// on EITHER end asks the human exactly like a read or a write would.
    ///
    /// Same-root copies run on the vendored `RootedFs::copy_file` (fully
    /// fenced, atomic at the destination). A cross-root copy cannot run on
    /// any single fence: the source is resolved and containment-checked
    /// through its own root, bounded by BOTH budgets (it is a read of the
    /// source and a write of the destination), and the bytes then go
    /// through the destination fence's atomic binary lane.
    pub(crate) fn copy_file(
        &self,
        from: impl AsRef<Path>,
        to: impl AsRef<Path>,
    ) -> HarnessResult<u64> {
        let (source, from_rel) = self.route(from.as_ref())?;
        let (destination, to_rel) = self.route(to.as_ref())?;
        if source.root() == destination.root() {
            return source.copy_file(from_rel, to_rel);
        }
        let canonical = source.resolve_existing(from_rel)?;
        let metadata = std::fs::metadata(&canonical).map_err(|error| {
            Failure::new(
                inbharat_harness_core::ErrorCode::FilesystemDenied,
                FailureClass::Resource,
                "fs.copy",
                "cannot read source metadata",
            )
            .with_detail("io_error", error.to_string())
        })?;
        if !metadata.is_file() {
            return Err(Failure::new(
                inbharat_harness_core::ErrorCode::FilesystemDenied,
                FailureClass::Policy,
                "fs.copy",
                "source is not a file (copying a directory tree is not supported)",
            ));
        }
        let bound = source.max_read_bytes().min(destination.max_write_bytes());
        if metadata.len() > u64::try_from(bound).unwrap_or(u64::MAX) {
            return Err(Failure::new(
                inbharat_harness_core::ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "fs.copy",
                "copy exceeds configured byte limit",
            ));
        }
        let bytes = std::fs::read(&canonical).map_err(|error| {
            Failure::new(
                inbharat_harness_core::ErrorCode::FilesystemDenied,
                FailureClass::Resource,
                "fs.copy",
                "cannot read source file",
            )
            .with_detail("io_error", error.to_string())
        })?;
        destination.write_bytes_atomic(to_rel, &bytes)?;
        Ok(bytes.len() as u64)
    }
}

/// Lexical containment check of an absolute path against one root, with the
/// same spellings `RootedFs` accepts: the `\\?\` verbatim prefix
/// `fs::canonicalize` produces, case variants (Windows filesystems are
/// case-insensitive), and `/`-vs-`\` separators. The boundary guard keeps
/// `C:\rootx` from counting as inside `C:\root`. Mirrors the vendored
/// `RootedFs::strip_root_prefix` (execution.rs) against the public root so
/// the desktop crate owns — and CI actually exercises — this matcher.
fn strip_root_prefix(root: &Path, path: &Path) -> Option<PathBuf> {
    if let Ok(stripped) = path.strip_prefix(root) {
        return Some(stripped.to_path_buf());
    }
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
        let root_raw = root.to_string_lossy();
        let root_raw = root_raw.strip_prefix(r"\\?\").unwrap_or(&root_raw);
        // Both replacements below are strictly length-preserving, so the
        // match offset still maps back onto the caller's original string
        // (models often spell Windows paths with forward slashes).
        let candidate = raw.to_ascii_lowercase().replace('/', "\\");
        let root = root_raw
            .to_ascii_lowercase()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_owned();
        let rest = candidate.strip_prefix(&root)?;
        // Boundary guard: the matched prefix must end at a component
        // boundary or `C:\rootx` would count as inside root `C:\root`.
        if !rest.is_empty() && !rest.starts_with('\\') {
            return None;
        }
        let offset = raw.len() - rest.len();
        let remainder = &raw[offset..];
        Some(PathBuf::from(remainder.trim_start_matches(['/', '\\'])))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The `ExecutionBroker` for a granted-folder run: file operations route
/// through [`GrantedFolders`]; process operations are the untouched
/// `LocalExecutionBroker` lane (same allowlist, same bounds, same
/// workspace working directory).
pub(crate) struct GrantedFolderBroker {
    folders: GrantedFolders,
    processes: crate::desktop_process::DesktopProcessBroker,
}

impl GrantedFolderBroker {
    #[cfg(test)]
    pub(crate) fn new(
        folders: GrantedFolders,
        allowed_programs: impl IntoIterator<Item = String>,
    ) -> Self {
        Self::with_process_lease(
            folders,
            allowed_programs,
            crate::desktop_process::DesktopProcessState::default().lease(),
        )
    }

    pub(crate) fn with_process_lease(
        folders: GrantedFolders,
        allowed_programs: impl IntoIterator<Item = String>,
        lease: crate::desktop_process::ProcessLease,
    ) -> Self {
        let processes = crate::desktop_process::DesktopProcessBroker::new(
            folders.primary().root(),
            allowed_programs,
            lease,
        );
        Self { folders, processes }
    }
}

impl ExecutionBroker for GrantedFolderBroker {
    fn world_id(&self) -> &str {
        "desktop-granted-folders-v1"
    }

    fn read_text(&self, relative: &Path) -> HarnessResult<String> {
        self.folders.read_text(relative)
    }

    fn list(&self, relative: &Path) -> HarnessResult<Vec<String>> {
        self.folders.list(relative)
    }

    fn write_text_atomic(&self, relative: &Path, contents: &str) -> HarnessResult<()> {
        self.folders.write_text_atomic(relative, contents)
    }

    fn create_dir_all(&self, relative: &Path) -> HarnessResult<()> {
        self.folders.create_dir_all(relative)
    }

    fn copy_file(&self, from: &Path, to: &Path) -> HarnessResult<u64> {
        self.folders.copy_file(from, to)
    }

    fn run_process(
        &self,
        spec: &ProcessSpec,
        cancel: &inbharat_harness_core::CancellationToken,
    ) -> HarnessResult<ProcessOutput> {
        self.processes.run_process(spec, cancel)
    }

    fn spawn_detached(
        &self,
        spec: &ProcessSpec,
        cancel: &inbharat_harness_core::CancellationToken,
    ) -> HarnessResult<DetachedSpawn> {
        self.processes.spawn_detached(spec, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inbharat_harness_core::RootedFs;
    use std::fs;

    /// Two granted temp folders plus an ungranted third, canonicalized the
    /// way `RootedFs::new` stores roots (Windows `\\?\` verbatim).
    struct Layout {
        workspace: PathBuf,
        extra: PathBuf,
        outside: PathBuf,
    }

    fn layout() -> Layout {
        let base = std::env::temp_dir().join(format!(
            "unoone-granted-fs-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let workspace = base.join("workspace");
        let extra = base.join("extra");
        let outside = base.join("outside");
        for dir in [&workspace, &extra, &outside] {
            fs::create_dir_all(dir).expect("test dirs");
        }
        Layout {
            workspace: fs::canonicalize(&workspace).expect("canonical"),
            extra: fs::canonicalize(&extra).expect("canonical"),
            outside: fs::canonicalize(&outside).expect("canonical"),
        }
    }

    fn folders(layout: &Layout) -> GrantedFolders {
        GrantedFolders::new(vec![
            RootedFs::new(&layout.workspace).expect("workspace fence"),
            RootedFs::new(&layout.extra).expect("extra fence"),
        ])
        .expect("granted set")
    }

    fn write(fenced: &RootedFs, relative: &str, contents: &str) {
        fenced
            .write_text_atomic(relative, contents)
            .expect("seed write");
    }

    #[test]
    fn relative_paths_resolve_against_the_workspace_root() {
        let layout = layout();
        let folders = folders(&layout);
        folders
            .write_text_atomic("notes.txt", "from the workspace")
            .expect("relative write");
        let text = folders.read_text("notes.txt").expect("relative read");
        assert_eq!(text, "from the workspace");
    }

    #[test]
    fn absolute_paths_route_to_the_containing_granted_folder() {
        let layout = layout();
        write(
            &RootedFs::new(&layout.extra).expect("fence"),
            "report.txt",
            "extra file",
        );
        let folders = folders(&layout);
        let absolute = layout.extra.join("report.txt");
        let text = folders.read_text(&absolute).expect("routed read");
        assert_eq!(text, "extra file");
        // A routed write lands in the right folder too.
        folders
            .write_text_atomic(layout.extra.join("made-by-agent.txt"), "routed")
            .expect("routed write");
        assert!(layout.extra.join("made-by-agent.txt").is_file());
        assert!(!layout.workspace.join("made-by-agent.txt").exists());
    }

    #[test]
    fn absolute_paths_outside_every_grant_are_denied_with_the_granted_list() {
        let layout = layout();
        let folders = folders(&layout);
        let absolute = layout.outside.join("never.txt");
        let error = folders
            .write_text_atomic(&absolute, "no")
            .expect_err("denied");
        assert_eq!(
            error.code,
            inbharat_harness_core::ErrorCode::FilesystemDenied
        );
        assert!(
            error
                .details
                .get("granted_folders")
                .is_some_and(|list| list.contains("workspace")),
            "denial names the granted folders"
        );
        assert!(!layout.outside.join("never.txt").exists());
    }

    #[test]
    fn nested_grants_resolve_to_the_longest_root() {
        let layout = layout();
        let nested = layout.extra.join("nested");
        fs::create_dir_all(&nested).expect("nested dir");
        let nested = fs::canonicalize(&nested).expect("canonical nested");
        let granted = GrantedFolders::new(vec![
            RootedFs::new(&layout.extra).expect("outer fence"),
            RootedFs::new(&nested).expect("inner fence"),
        ])
        .expect("granted set");
        // The nested folder matches BOTH roots; the longer root must win,
        // observable through the byte limits of the inner fence.
        granted
            .write_text_atomic(nested.join("deep.txt"), "inner")
            .expect("nested write");
        assert!(nested.join("deep.txt").is_file());
    }

    #[test]
    fn boundary_root_prefixes_do_not_match_sibling_folders() {
        let layout = layout();
        // `...\extra` granted, `...\extrax` not: the boundary guard must
        // refuse the sibling even though it shares the prefix string.
        let sibling = layout.extra.with_file_name("extrax");
        fs::create_dir_all(&sibling).expect("sibling dir");
        let folders = folders(&layout);
        assert!(
            folders.write_text_atomic(&sibling, "no").is_err(),
            "a sibling sharing the prefix string must not be routed as inside"
        );
        assert!(!sibling.join("no").exists());
    }

    #[test]
    fn case_and_separator_and_verbatim_spellings_are_accepted() {
        let layout = layout();
        write(
            &RootedFs::new(&layout.extra).expect("fence"),
            "doc.txt",
            "cased",
        );
        let folders = folders(&layout);
        let raw = layout.extra.to_string_lossy().to_string();
        // Windows-only spellings; on other hosts the canonical form is
        // already exact, so the test checks that form instead.
        #[cfg(windows)]
        let variants = {
            let plain = raw.trim_start_matches(r"\\?\").replace('\\', "/");
            let lower = plain.to_ascii_lowercase().replace('/', "\\");
            vec![PathBuf::from(&plain), PathBuf::from(&lower)]
        };
        #[cfg(not(windows))]
        let variants = vec![layout.extra.clone()];
        for variant in variants {
            let text = folders
                .read_text(variant.join("doc.txt"))
                .unwrap_or_else(|_| panic!("spelling must route: {}", variant.display()));
            assert_eq!(text, "cased");
        }
    }

    #[test]
    fn broker_routes_file_operations_and_keeps_the_process_lane() {
        let layout = layout();
        let folders = folders(&layout);
        let broker = GrantedFolderBroker::new(folders, ["git".to_owned()]);
        assert_eq!(broker.world_id(), "desktop-granted-folders-v1");
        broker
            .write_text_atomic(Path::new("via-broker.txt"), "primary")
            .expect("broker write");
        broker
            .write_text_atomic(&layout.extra.join("via-broker.txt"), "extra")
            .expect("broker routed write");
        assert_eq!(
            broker
                .read_text(Path::new("via-broker.txt"))
                .expect("primary read"),
            "primary"
        );
        assert_eq!(
            broker
                .read_text(&layout.extra.join("via-broker.txt"))
                .expect("routed read"),
            "extra"
        );
        // The process lane stays the untouched LocalExecutionBroker path:
        // a non-allowlisted program is refused exactly as before.
        let denied = broker
            .run_process(
                &ProcessSpec {
                    program: "definitely-not-allowlisted".to_owned(),
                    args: vec![],
                    environment: std::collections::BTreeMap::new(),
                    timeout: std::time::Duration::from_secs(5),
                    max_output_bytes: 8 * 1024,
                },
                &inbharat_harness_core::CancellationToken::new(),
            )
            .expect_err("not allowlisted");
        assert_eq!(
            denied.code,
            inbharat_harness_core::ErrorCode::SubprocessDenied
        );
    }

    #[test]
    fn an_empty_grant_set_is_refused() {
        assert!(GrantedFolders::new(vec![]).is_err());
    }

    // -- binary-safe copy (design-website lane, 2026-10-03) --

    #[test]
    fn copy_file_round_trips_binary_same_root_and_cross_root() {
        // A PNG-shaped payload (high-bit bytes, NOT valid UTF-8) must arrive
        // byte-identical whether the copy stays inside one granted folder or
        // crosses from one grant into another — the text lane corrupts
        // exactly this content, which is why fs.copy exists.
        let layout = layout();
        let payload: Vec<u8> = (0..=255_u16)
            .map(|index| index as u8 ^ 0xA5)
            .chain([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
            .collect();
        let extra_fence = RootedFs::new(&layout.extra).expect("extra fence");
        extra_fence
            .write_bytes_atomic("assets/photo.png", &payload)
            .expect("seed binary");

        let folders = folders(&layout);
        // Same-root: inside the extra grant.
        let copied = folders
            .copy_file(
                layout.extra.join("assets").join("photo.png"),
                layout.extra.join("site").join("images").join("photo.png"),
            )
            .expect("same-root copy");
        assert_eq!(copied, payload.len() as u64);
        assert_eq!(
            fs::read(layout.extra.join("site").join("images").join("photo.png"))
                .expect("re-read same-root copy"),
            payload
        );
        // Cross-root: extra grant → workspace grant.
        let copied = folders
            .copy_file(
                layout.extra.join("assets").join("photo.png"),
                layout.workspace.join("site").join("photo.png"),
            )
            .expect("cross-root copy");
        assert_eq!(copied, payload.len() as u64);
        assert_eq!(
            fs::read(layout.workspace.join("site").join("photo.png"))
                .expect("re-read cross-root copy"),
            payload
        );

        // Through the broker the model actually calls.
        let broker = GrantedFolderBroker::new(folders, Vec::<String>::new());
        let copied = broker
            .copy_file(
                Path::new(&layout.extra.join("assets").join("photo.png")),
                Path::new(&layout.workspace.join("via-broker.png")),
            )
            .expect("broker copy");
        assert_eq!(copied, payload.len() as u64);
        assert_eq!(
            fs::read(layout.workspace.join("via-broker.png")).expect("re-read broker copy"),
            payload
        );
    }

    #[test]
    fn copy_file_denies_outside_on_either_end_without_creating_anything() {
        let layout = layout();
        let folders = folders(&layout);
        fs::write(layout.outside.join("seed.png"), b"seeded").expect("seed outside");
        // Outside SOURCE: denied (and the in-chat hook is absent here, so
        // the denial stands exactly like a read refusal).
        let error = folders
            .copy_file(
                layout.outside.join("seed.png"),
                layout.workspace.join("stolen.png"),
            )
            .expect_err("outside source");
        assert_eq!(
            error.code,
            inbharat_harness_core::ErrorCode::FilesystemDenied
        );
        assert!(!layout.workspace.join("stolen.png").exists());
        // Outside DESTINATION: denied, nothing escapes the union.
        fs::write(layout.workspace.join("real.png"), b"real").expect("seed workspace");
        let error = folders
            .copy_file(
                layout.workspace.join("real.png"),
                layout.outside.join("leak.png"),
            )
            .expect_err("outside destination");
        assert_eq!(
            error.code,
            inbharat_harness_core::ErrorCode::FilesystemDenied
        );
        assert!(!layout.outside.join("leak.png").exists());
    }

    // -- in-chat approval hook (2026-10-03) --

    /// A hook that "grants" the outside folder on first ask — the shape
    /// `request_folder_grant` returns after a human clicks Grant.
    #[test]
    fn a_granted_denied_path_routes_through_the_hook() {
        let layout = layout();
        let folders = folders(&layout);
        let hooked = folders.clone().with_denied_request({
            let outside_root = RootedFs::new(&layout.outside).expect("outside fence");
            let expected = layout.outside.join("asked.txt");
            std::sync::Arc::new(move |path: &Path| {
                assert_eq!(path, &expected);
                DeniedPathResolution::Granted(outside_root.clone(), PathBuf::from("asked.txt"))
            })
        });
        hooked
            .write_text_atomic(layout.outside.join("asked.txt"), "after grant")
            .expect("hook-granted write");
        assert_eq!(
            std::fs::read_to_string(layout.outside.join("asked.txt")).expect("written"),
            "after grant"
        );
        // The plain denial without a hook keeps failing closed.
        folders
            .write_text_atomic(layout.outside.join("plain.txt"), "no")
            .expect_err("no hook, no grant");
        assert!(!layout.outside.join("plain.txt").exists());
    }

    #[test]
    fn a_declined_denied_path_names_the_reason_the_model_sees() {
        let layout = layout();
        let folders = folders(&layout).with_denied_request(std::sync::Arc::new(|_| {
            DeniedPathResolution::Denied("the user declined to grant the folder".to_owned())
        }));
        let error = folders
            .read_text(layout.outside.join("secret.txt"))
            .expect_err("declined");
        assert_eq!(
            error.code,
            inbharat_harness_core::ErrorCode::FilesystemDenied
        );
        assert!(
            error
                .message
                .contains("the user declined to grant the folder"),
            "message carries the denial reason: {}",
            error.message
        );
        assert!(
            error.message.contains("secret.txt"),
            "message names the path: {}",
            error.message
        );
    }

    #[test]
    fn a_relative_path_denial_never_asks_the_human() {
        let layout = layout();
        // A relative path escaping the root is a containment refusal, not a
        // missing grant: the hook must never fire for it.
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&asked);
        let folders = folders(&layout).with_denied_request(std::sync::Arc::new(move |_| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            DeniedPathResolution::Denied("must not be asked".to_owned())
        }));
        let escaped = folders.read_text("../outside/escape.txt");
        assert!(escaped.is_err());
        assert!(
            !asked.load(std::sync::atomic::Ordering::SeqCst),
            "the hook must not fire for relative paths"
        );
    }

    #[test]
    fn try_route_absolute_never_fires_the_hook() {
        let layout = layout();
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&fired);
        let folders = folders(&layout).with_denied_request(std::sync::Arc::new(move |_| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            DeniedPathResolution::Denied("must not be asked".to_owned())
        }));
        assert!(folders
            .try_route_absolute(&layout.extra.join("x.txt"))
            .is_some());
        assert!(folders
            .try_route_absolute(&layout.outside.join("x.txt"))
            .is_none());
        assert!(
            !fired.load(std::sync::atomic::Ordering::SeqCst),
            "the fresh-store re-check must not recurse through the hook"
        );
    }
}
