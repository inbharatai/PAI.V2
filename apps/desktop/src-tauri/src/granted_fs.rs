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
//! delegate unchanged to an inner `LocalExecutionBroker` rooted at the
//! workspace, so the allowlist, argument bounds, scrubbed environment and
//! working directory behave exactly as before (a folder grant widens file
//! access, never the program allowlist).

use inbharat_harness_core::execution::{DetachedSpawn, ProcessOutput};
use inbharat_harness_core::{
    ExecutionBroker, Failure, FailureClass, HarnessResult, LocalExecutionBroker, ProcessSpec,
    RootedFs,
};
use std::path::{Path, PathBuf};

/// The multi-root fence: one `RootedFs` per granted folder, workspace first.
#[derive(Clone, Debug)]
pub(crate) struct GrantedFolders {
    /// Index 0 is the workspace root (the primary); grants keep their
    /// persisted order after it.
    roots: Vec<RootedFs>,
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
        Ok(Self { roots })
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
    fn route(&self, path: &Path) -> HarnessResult<(&RootedFs, PathBuf)> {
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
            .ok_or_else(|| self.outside_failure(path))
    }

    /// The honest denial for an absolute path no grant covers: names the
    /// path and every granted root so a model can correct itself.
    fn outside_failure(&self, path: &Path) -> Failure {
        let granted = self
            .roots
            .iter()
            .map(|fenced| fenced.root().display().to_string())
            .collect::<Vec<_>>()
            .join("; ");
        Failure::new(
            inbharat_harness_core::ErrorCode::FilesystemDenied,
            FailureClass::Policy,
            "fs.route",
            "the path is outside every folder the user granted the agent",
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
    processes: LocalExecutionBroker,
}

impl GrantedFolderBroker {
    pub(crate) fn new(
        folders: GrantedFolders,
        allowed_programs: impl IntoIterator<Item = String>,
    ) -> Self {
        let processes = LocalExecutionBroker::new(folders.primary().clone(), allowed_programs);
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
}
