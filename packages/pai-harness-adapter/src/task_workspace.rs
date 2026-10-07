//! Stage 5 isolated working set and path policy (design §4.1, owner B).
//!
//! The working set is the ONLY mutable copy of the user-selected files during a
//! coding task. It lives in memory (and, through owner A, as encrypted ledger
//! blobs); nothing in this module writes the host filesystem. The only host read
//! is the bounded, fd-safe `.git` metadata read behind [`repository_label`],
//! which is labelled `GitFilesUnverified` and never authenticates anything.
//!
//! Path policy (identical for copy-in selection checks, edits and sandbox
//! copy-out): canonical relative names only (non-empty, ≤256 bytes, ≤8
//! components, no `.`/`..`/empty component, no `\`, NUL, control, invisible
//! format/bidi characters or Windows-reserved characters, no trailing dot or
//! space), plus a case-folded deny list (`.git`, `.hg`, `.svn`, `.env*`, keys and
//! credential stores, `node_modules`, `__pycache__`, `.pai-*`, NTFS short-name
//! aliases, Windows device names). Oracle (protected test) files, including the
//! test helpers and data they use (see [`derive_oracle_set`]), can never be
//! edited, deleted, replaced by copy-out, shadowed by a case variant, joined by
//! new files in their directories, or shadowed by another Python import source
//! for the same module name (a new `X/` package or namespace directory or
//! `X.pyc`/`.so`/`.pyd`/`.pyw` for a protected `X.py`; a new sibling `X.py` for
//! a protected package `X/__init__.py` or module below `X/`), in direct edits
//! and sandbox copy-out alike. New files are allowed only below the
//! user-confirmed `allowed_new_prefixes` (each must end in `/`, so `src/` never
//! admits `src-evil/`). Binary (non-UTF-8 or NUL-containing) files are view-only.

use crate::isolation::{hash, SnapshotPolicy};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};

pub(crate) use crate::isolation::workspace::{
    manifest_sha256, read_metadata, CapturedSelection, CopyOutMode, CopyOutReject, CopyOutReport,
    ReportedFile,
};
pub(crate) use crate::task_ledger::{LabelSource, RepositoryLabel};

/// Hard caps; `PathPolicy` values above them are clamped down, never up.
pub const HARD_MAX_FILES: usize = 32;
pub const HARD_MAX_FILE_BYTES: usize = 256 * 1024;
pub const HARD_MAX_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 256;
pub const MAX_PATH_DEPTH: usize = 8;
pub const MAX_NEW_PREFIXES: usize = 8;
/// Upper bound on reported copy-out entries the host will even look at.
pub const MAX_COPY_OUT_ENTRIES: usize = 64;
const MAX_PATCH_HUNKS: usize = 1024;
const MAX_PATCH_LINES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditDenial {
    InvalidPath,
    DeniedPattern,
    OracleProtected,
    NotSelected,
    NewPathNotAllowed,
    TooLarge,
    TooMany,
    NotUtf8,
    PatchContextMismatch,
    /// Added to the frozen §4.1 set (handoff note): host-recomputed copy-out
    /// SHA-256 or size differs from the sandbox report, or the digest is malformed.
    HashMismatch,
    /// Added to the frozen §4.1 set (handoff note): the sandbox reported a
    /// symlink or special file (never followed, never applied).
    NotRegularFile,
}
impl fmt::Display for EditDenial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately content-free: denials never echo file bytes or paths.
        write!(f, "edit denied: {self:?}")
    }
}
impl std::error::Error for EditDenial {}

/// A proposed change to the working set. Model, user and repair proposers all
/// produce this; it is DATA and is always re-validated against [`PathPolicy`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposedEdit {
    Replace {
        path: String,
        content: String,
    },
    /// Unified hunks (optional `--- a/<path>` / `+++ b/<path>` headers), applied
    /// with EXACT context and exact line numbers: no fuzz, no offset search.
    Patch {
        path: String,
        unified_hunks: String,
    },
    Create {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
}
impl ProposedEdit {
    pub fn path(&self) -> &str {
        match self {
            Self::Replace { path, .. }
            | Self::Patch { path, .. }
            | Self::Create { path, .. }
            | Self::Delete { path } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathPolicy {
    pub selected: BTreeSet<String>,
    pub oracle: BTreeSet<String>,
    /// ≤ 8, user-confirmed, each a canonical directory prefix ending in `/`.
    pub allowed_new_prefixes: Vec<String>,
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}

/// Why a file is in the derived oracle set (shown to the user with the set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OracleReason {
    Declared,
    /// Any selected file inside a (non-root) directory holding a declared oracle.
    InOracleDirectory,
    /// `conftest.py` in a directory that is an ancestor of an oracle file.
    ConftestAncestor,
    /// Imported (transitively) by an oracle file and helper-named or inside an
    /// oracle directory.
    ImportedHelper {
        by: String,
    },
    /// Non-Python selected file whose name is quoted in an oracle file.
    ReferencedData {
        by: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleDerivation {
    pub oracle: BTreeSet<String>,
    pub reasons: BTreeMap<String, OracleReason>,
}

impl PathPolicy {
    /// Default Stage 5 bounds (32 files, 256 KiB per file, 2 MiB total).
    pub fn new(
        selected: BTreeSet<String>,
        oracle: BTreeSet<String>,
        allowed_new_prefixes: Vec<String>,
    ) -> Self {
        Self {
            selected,
            oracle,
            allowed_new_prefixes,
            max_files: HARD_MAX_FILES,
            max_file_bytes: HARD_MAX_FILE_BYTES,
            max_total_bytes: HARD_MAX_TOTAL_BYTES,
        }
    }
    /// Policy for a captured selection, with the oracle set DERIVED from the
    /// declared protected tests (helpers and data they use are added; Stage 4
    /// signoff requirement). The derivation is returned for display.
    pub fn for_selection(
        files: &BTreeMap<String, Vec<u8>>,
        declared_oracle: &BTreeSet<String>,
        allowed_new_prefixes: Vec<String>,
    ) -> (Self, OracleDerivation) {
        let derivation = derive_oracle_set(files, declared_oracle);
        let policy = Self::new(
            files.keys().cloned().collect(),
            derivation.oracle.clone(),
            allowed_new_prefixes,
        );
        (policy, derivation)
    }
    /// Validates the selection itself (HA31: an invalid or denied selection is
    /// refused before capture, without reading any bytes).
    pub fn check_selection(&self) -> Result<(), (String, EditDenial)> {
        if self.selected.is_empty() || self.selected.len() > self.limit_files() {
            return Err((String::new(), EditDenial::TooMany));
        }
        for path in self.selected.iter().chain(self.oracle.iter()) {
            validate_path(path).map_err(|e| (path.clone(), e))?;
        }
        let mut folded = BTreeMap::new();
        for path in &self.selected {
            if let Some(other) = folded.insert(fold(path), path) {
                if other != path {
                    return Err((path.clone(), EditDenial::InvalidPath));
                }
            }
        }
        if !self.prefixes_valid() {
            return Err((String::new(), EditDenial::NewPathNotAllowed));
        }
        Ok(())
    }
    fn limit_files(&self) -> usize {
        self.max_files.min(HARD_MAX_FILES)
    }
    fn limit_file_bytes(&self) -> usize {
        self.max_file_bytes.min(HARD_MAX_FILE_BYTES)
    }
    fn limit_total_bytes(&self) -> usize {
        self.max_total_bytes.min(HARD_MAX_TOTAL_BYTES)
    }
    fn prefixes_valid(&self) -> bool {
        self.allowed_new_prefixes.len() <= MAX_NEW_PREFIXES
            && self.allowed_new_prefixes.iter().all(|p| {
                p.len() > 1
                    && p.ends_with('/')
                    && validate_path(&p[..p.len() - 1]).is_ok()
                    && p.split('/').count() <= MAX_PATH_DEPTH
            })
    }
    /// Component-wise, case-sensitive prefix match; any malformed prefix denies
    /// every new path (fail closed).
    fn prefix_allows(&self, path: &str) -> bool {
        self.prefixes_valid()
            && self
                .allowed_new_prefixes
                .iter()
                .any(|p| path.starts_with(p.as_str()) && path.len() > p.len())
    }
    /// Case-folded: a case variant of an oracle name is the oracle.
    pub fn is_oracle(&self, path: &str) -> bool {
        let f = fold(path);
        self.oracle.iter().any(|o| fold(o) == f)
    }
    /// A ROOT-level oracle file has no oracle directory; its Python aliases are
    /// covered by [`aliases_oracle_module`](Self::aliases_oracle_module).
    fn in_oracle_dir(&self, path: &str) -> bool {
        let f = fold(path);
        self.oracle.iter().any(|o| match o.rsplit_once('/') {
            Some((dir, _)) => f.starts_with(&(fold(dir) + "/")),
            None => false,
        })
    }
    /// Python module-name aliasing (R3 L1 / Q5 d7), case-folded: a NEW `path`
    /// must not be another import source for the module name of a protected
    /// Python file (see [`module_name`]) or for a package directory holding one:
    /// * nothing below `D/X/` when `D/X.py` is protected (a regular package
    ///   `D/X/__init__.py` is imported instead of the module; a namespace
    ///   portion or nested entry is refused too);
    /// * no module file `D/X.<py|pyw|pyc|pyo|so|pyd>` when `D/X.py` is protected
    ///   (another loader for the same name), when the package `D/X/__init__.py`
    ///   is protected, or when any protected module lies below `D/X/` (a module
    ///   shadows a namespace package and can redirect `__path__`).
    ///
    /// The same stem in ANOTHER directory (`src/X.py` or `src/X/__init__.py`
    /// for a protected root `X.py`) imports under a different name (`src.X`)
    /// and stays allowed, like the R3 d6/c5 controls: besides the protected
    /// oracle directory only `/work` is on the oracle's `sys.path`.
    fn aliases_oracle_module(&self, path: &str) -> bool {
        let f = fold(path);
        let own = module_name(&f);
        self.oracle.iter().any(|o| {
            let o = fold(o);
            module_name(&o).is_some_and(|m| {
                f.starts_with(&format!("{m}/"))
                    || own
                        .as_deref()
                        .is_some_and(|n| n == m || o.starts_with(&format!("{n}/")))
            })
        })
    }
}

/// Import name (as a `/`-joined path of the case-folded input) that a
/// Python-importable file provides: `D/X.py`, `.pyw`, `.pyc`, `.pyo` or an
/// extension module `D/X[.tag].so` / `D/X[.tag].pyd` -> `D/X`; a package
/// `D/X/__init__.<same>` -> `D/X`. `None` for any other file, for a dotted
/// stem (not importable under that name) and for a root `__init__`.
fn module_name(folded: &str) -> Option<String> {
    let (dir, name) = match folded.rsplit_once('/') {
        Some((d, n)) => (Some(d), n),
        None => (None, folded),
    };
    let stem = [".py", ".pyw", ".pyc", ".pyo"]
        .iter()
        .find_map(|&s| name.strip_suffix(s))
        .or_else(|| {
            [".so", ".pyd"]
                .iter()
                .find_map(|&s| name.strip_suffix(s))
                .and_then(|n| n.split('.').next())
        })
        .filter(|s| !s.is_empty() && !s.contains('.'))?;
    match (stem, dir) {
        ("__init__", d) => d.map(str::to_owned),
        (s, Some(d)) => Some(format!("{d}/{s}")),
        (s, None) => Some(s.to_owned()),
    }
}

/// Canonical relative name + deny list. Applied everywhere a path enters.
pub fn validate_path(path: &str) -> Result<(), EditDenial> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return Err(EditDenial::InvalidPath);
    }
    if path.chars().any(|c| {
        c == '\\'
            || c == '\0'
            || c.is_control()
            || invisible(c)
            || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')
    }) {
        return Err(EditDenial::InvalidPath);
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_PATH_DEPTH
        || parts.iter().any(|c| {
            c.is_empty() || *c == "." || *c == ".." || c.ends_with('.') || c.ends_with(' ')
        })
        || !Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(EditDenial::InvalidPath);
    }
    if parts.iter().any(|c| denied_component(&fold(c))) {
        return Err(EditDenial::DeniedPattern);
    }
    Ok(())
}

/// Invisible / format / bidi-control code points: they enable look-alike names
/// (review spoofing, "Trojan Source"-style file names) and HFS+ ignorable aliases.
fn invisible(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F
        | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F
        | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFB | 0xE0000..=0xE0FFF)
}

/// Full case fold for comparisons (`ı`→`i`, Kelvin `K`→`k`, `ſ`→`s`): anticipates
/// case-insensitive filesystems. Windows aliasing itself stays untested here.
pub(crate) fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect()
}

const DENIED_EXACT: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".env",
    ".envrc",
    "node_modules",
    "__pycache__",
    ".pytest_cache",
    ".ssh",
    ".gnupg",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".aws",
    ".docker",
    ".kube",
];
const DENIED_PREFIX: &[&str] = &[
    ".env.",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".pai-",
];
const DENIED_SUFFIX: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".kdbx"];
const WINDOWS_DEVICES: &[&str] = &["con", "prn", "aux", "nul", "conin$", "conout$", "clock$"];

fn denied_component(folded: &str) -> bool {
    if DENIED_EXACT.contains(&folded)
        || DENIED_PREFIX.iter().any(|p| folded.starts_with(p))
        || DENIED_SUFFIX.iter().any(|s| folded.ends_with(s))
    {
        return true;
    }
    // NTFS 8.3 short-name aliases of denied directories (GIT~1 == .git).
    for alias in ["git~", "hg~", "svn~", "env~"] {
        if let Some(rest) = folded.strip_prefix(alias) {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                return true;
            }
        }
    }
    let stem = folded.split('.').next().unwrap_or(folded);
    if WINDOWS_DEVICES.contains(&stem) {
        return true;
    }
    for dev in ["com", "lpt"] {
        if let Some(rest) = stem.strip_prefix(dev) {
            let mut chars = rest.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³') {
                    return true;
                }
            }
        }
    }
    false
}

/// Diffable/appliable text: valid UTF-8 without NUL bytes.
pub fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Names that would join or steer the oracle's own process when created:
/// unittest/pytest discovery modules, pytest configuration, interpreter
/// startup hooks.
fn test_support_name(path: &str) -> bool {
    let b = fold(basename(path));
    (b.ends_with(".py")
        && (b.starts_with("test") || b.ends_with("_test.py") || b.ends_with("_tests.py")))
        || b.ends_with(".pth")
        || [
            "conftest.py",
            "sitecustomize.py",
            "usercustomize.py",
            "pytest.ini",
            "tox.ini",
            "setup.cfg",
            "pyproject.toml",
        ]
        .contains(&b.as_str())
}

fn helper_name(path: &str) -> bool {
    let b = fold(basename(path));
    test_support_name(path)
        || [
            "helper", "fixture", "testutil", "testing", "mock", "fake", "stub", "oracle", "expect",
        ]
        .iter()
        .any(|k| b.contains(k))
}

/// The protected set for a task: declared oracle files plus the test helpers and
/// data they rely on. Over-inclusion only makes more files read-only (shown to
/// the user); under-inclusion is the Stage 4 B3 residual this narrows.
pub fn derive_oracle_set(
    files: &BTreeMap<String, Vec<u8>>,
    declared: &BTreeSet<String>,
) -> OracleDerivation {
    let mut reasons: BTreeMap<String, OracleReason> = declared
        .iter()
        .map(|p| (p.clone(), OracleReason::Declared))
        .collect();
    let dirs: Vec<String> = declared
        .iter()
        .filter_map(|p| p.rsplit_once('/').map(|(d, _)| fold(d) + "/"))
        .collect();
    let in_dirs = |p: &str| {
        let f = fold(p);
        dirs.iter().any(|d| f.starts_with(d.as_str()))
    };
    for path in files.keys() {
        if reasons.contains_key(path) {
            continue;
        }
        if in_dirs(path) {
            reasons.insert(path.clone(), OracleReason::InOracleDirectory);
        } else if fold(basename(path)) == "conftest.py" {
            let dir = path
                .rsplit_once('/')
                .map_or(String::new(), |(d, _)| d.to_owned() + "/");
            if declared.iter().any(|o| o.starts_with(dir.as_str())) {
                reasons.insert(path.clone(), OracleReason::ConftestAncestor);
            }
        }
    }
    let mut queue: Vec<String> = reasons.keys().cloned().collect();
    while let Some(importer) = queue.pop() {
        let Some(src) = files
            .get(&importer)
            .and_then(|b| std::str::from_utf8(b).ok())
        else {
            continue;
        };
        for (level, module, names) in python_imports(src) {
            for cand in module_candidates(&importer, level, &module, &names) {
                if files.contains_key(&cand)
                    && !reasons.contains_key(&cand)
                    && (in_dirs(&cand) || helper_name(&cand))
                {
                    reasons.insert(
                        cand.clone(),
                        OracleReason::ImportedHelper {
                            by: importer.clone(),
                        },
                    );
                    queue.push(cand);
                }
            }
        }
        for path in files.keys() {
            if path.ends_with(".py") || reasons.contains_key(path) {
                continue;
            }
            let name = basename(path);
            if [path.as_str(), name]
                .iter()
                .any(|n| src.contains(&format!("\"{n}\"")) || src.contains(&format!("'{n}'")))
            {
                reasons.insert(
                    path.clone(),
                    OracleReason::ReferencedData {
                        by: importer.clone(),
                    },
                );
            }
        }
    }
    OracleDerivation {
        oracle: reasons.keys().cloned().collect(),
        reasons,
    }
}

/// Conservative line-based Python import scan: `import a.b`, `from .x import y`,
/// parenthesized/continued lists, `importlib.import_module("m")`, `__import__("m")`.
fn python_imports(src: &str) -> Vec<(usize, String, Vec<String>)> {
    let mut statements = Vec::new();
    let mut pending: Option<String> = None;
    let mut in_parens = false;
    for raw in src.lines() {
        for call in ["import_module(", "__import__("] {
            let mut rest = raw;
            while let Some(at) = rest.find(call) {
                rest = &rest[at + call.len()..];
                let trimmed = rest.trim_start();
                if let Some(q) = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'') {
                    if let Some(end) = trimmed[1..].find(q) {
                        statements.push(format!("import {}", &trimmed[1..1 + end]));
                    }
                }
            }
        }
        let line = raw.split('#').next().unwrap_or("").trim();
        if let Some(p) = pending.as_mut() {
            p.push(' ');
            p.push_str(line.trim_end_matches('\\'));
            let more = if in_parens {
                !line.contains(')')
            } else {
                line.ends_with('\\')
            };
            if !more {
                statements.extend(pending.take());
            }
            continue;
        }
        if line.starts_with("import ") || line.starts_with("from ") {
            if line.contains('(') && !line.contains(')') {
                pending = Some(line.to_owned());
                in_parens = true;
            } else if line.ends_with('\\') {
                pending = Some(line.trim_end_matches('\\').to_owned());
                in_parens = false;
            } else {
                statements.push(line.to_owned());
            }
        }
    }
    statements.extend(pending);
    let mut out = Vec::new();
    for statement in statements.iter().flat_map(|s| s.split(';')) {
        let s = statement.trim().replace(['(', ')'], " ");
        if let Some(list) = s.strip_prefix("import ") {
            for part in list.split(',') {
                if let Some(m) = part.split_whitespace().next() {
                    out.push((0, m.to_owned(), Vec::new()));
                }
            }
        } else if let Some(rest) = s.strip_prefix("from ") {
            if let Some((module, names)) = rest.split_once(" import ") {
                let module = module.trim();
                let level = module.chars().take_while(|c| *c == '.').count();
                let names = names
                    .split(',')
                    .filter_map(|n| n.split_whitespace().next())
                    .filter(|n| *n != "*")
                    .map(str::to_owned)
                    .collect();
                out.push((level, module[level..].to_owned(), names));
            }
        }
    }
    out
}

fn module_candidates(importer: &str, level: usize, module: &str, names: &[String]) -> Vec<String> {
    let dir: Vec<&str> = importer.split('/').collect();
    let dir = &dir[..dir.len() - 1];
    let bases: Vec<Vec<&str>> = if level == 0 {
        vec![Vec::new(), dir.to_vec()]
    } else if level - 1 <= dir.len() {
        vec![dir[..dir.len() - (level - 1)].to_vec()]
    } else {
        Vec::new()
    };
    let module: Vec<&str> = module.split('.').filter(|m| !m.is_empty()).collect();
    let mut out = Vec::new();
    for base in bases {
        let mut m = base.clone();
        m.extend(module.iter().copied());
        let mut push = |parts: &[&str]| {
            if !parts.is_empty() {
                let joined = parts.join("/");
                out.push(format!("{joined}.py"));
                out.push(format!("{joined}/__init__.py"));
            }
        };
        push(&m);
        for name in names {
            let mut n = m.clone();
            n.push(name);
            push(&n);
        }
    }
    out
}

/// Content manifest (path → lowercase SHA-256) of base and current, with the
/// canonical `manifest_sha256` identities used by StagedTree and the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingSetManifest {
    pub base: BTreeMap<String, String>,
    pub current: BTreeMap<String, String>,
    pub base_sha256: String,
    pub current_sha256: String,
}

/// Captured base + current bytes. `Debug` prints hashes only (no content).
#[derive(Clone, PartialEq, Eq)]
pub struct WorkingSet {
    base: BTreeMap<String, Vec<u8>>,
    current: BTreeMap<String, Vec<u8>>,
}
impl fmt::Debug for WorkingSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.manifest();
        f.debug_struct("WorkingSet")
            .field("base_sha256", &m.base_sha256)
            .field("current_sha256", &m.current_sha256)
            .field("files", &m.current.len())
            .finish()
    }
}

fn digests(files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, String> {
    files.iter().map(|(p, b)| (p.clone(), hash(b))).collect()
}

impl WorkingSet {
    pub fn from_capture(c: &CapturedSelection) -> Self {
        Self {
            base: c.files.clone(),
            current: c.files.clone(),
        }
    }
    /// Rebuild from ledger blobs (owner A, after restart). Paths and the hard
    /// caps are re-validated; the caller must compare `manifest()` with the
    /// sealed ledger manifest before use.
    pub fn from_parts(
        base: BTreeMap<String, Vec<u8>>,
        current: BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, EditDenial> {
        for map in [&base, &current] {
            if map.len() > HARD_MAX_FILES {
                return Err(EditDenial::TooMany);
            }
            let mut total = 0usize;
            for (path, bytes) in map {
                validate_path(path)?;
                if bytes.len() > HARD_MAX_FILE_BYTES {
                    return Err(EditDenial::TooLarge);
                }
                total += bytes.len();
            }
            if total > HARD_MAX_TOTAL_BYTES {
                return Err(EditDenial::TooLarge);
            }
        }
        Ok(Self { base, current })
    }
    pub fn manifest(&self) -> WorkingSetManifest {
        let base = digests(&self.base);
        let current = digests(&self.current);
        WorkingSetManifest {
            base_sha256: manifest_sha256(&base),
            current_sha256: manifest_sha256(&current),
            base,
            current,
        }
    }
    /// Current content → `StagedTree::from_files`.
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.current
    }
    /// Captured base content (for diffs and revert).
    pub fn base_files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.base
    }
    /// Paths whose current content differs from base (added, modified, deleted).
    pub fn changed_paths(&self) -> Vec<String> {
        self.base
            .keys()
            .chain(self.current.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|p| self.base.get(*p) != self.current.get(*p))
            .cloned()
            .collect()
    }

    /// Validates and applies one edit atomically (nothing changes on denial).
    /// Returns the changed path, or nothing when the content is identical.
    pub fn apply_edit(
        &mut self,
        policy: &PathPolicy,
        e: &ProposedEdit,
    ) -> Result<Vec<String>, EditDenial> {
        let limit = policy.limit_file_bytes();
        let (path, next) = match e {
            ProposedEdit::Replace { path, content } => {
                self.check_existing(policy, path)?;
                (path, Some(text_bytes(content, limit)?))
            }
            ProposedEdit::Patch {
                path,
                unified_hunks,
            } => {
                self.check_existing(policy, path)?;
                if unified_hunks.len() > limit.saturating_mul(4) {
                    return Err(EditDenial::TooLarge);
                }
                let current = std::str::from_utf8(&self.current[path.as_str()])
                    .map_err(|_| EditDenial::NotUtf8)?;
                let patched = apply_unified(current, path, unified_hunks)?;
                (path, Some(text_bytes(&patched, limit)?))
            }
            ProposedEdit::Create { path, content } => {
                self.check_new(policy, path)?;
                (path, Some(text_bytes(content, limit)?))
            }
            ProposedEdit::Delete { path } => {
                self.check_existing(policy, path)?;
                (path, None)
            }
        };
        self.commit(policy, path, next)
    }

    /// Sandbox copy-out (design §4.1): every reported path is re-checked against
    /// the policy, every content SHA-256 and size is recomputed on the host and
    /// must equal the report, sizes/counts are bounded, and symlinks/specials
    /// are reported as denials. Only `CopyOutMode::Contents` can change the
    /// working set; other modes only surface policy denials (e.g. an oracle
    /// rewritten inside the sandbox). Nothing here reaches the host filesystem.
    /// The caller must ensure the gate ran on the CURRENT manifest
    /// (`GateRunResult.tree_sha256 == manifest().current_sha256`) — see
    /// [`WorkingSet::apply_copy_out_checked`].
    pub fn apply_copy_out(
        &mut self,
        policy: &PathPolicy,
        r: &CopyOutReport,
        mode: CopyOutMode,
    ) -> (Vec<String>, Vec<(String, EditDenial)>) {
        let contents = matches!(mode, CopyOutMode::Contents);
        let mut applied = Vec::new();
        let mut denied = Vec::new();
        let total = r.changed.len() + r.created.len() + r.deleted.len() + r.rejected.len();
        if total > MAX_COPY_OUT_ENTRIES * 2 {
            denied.push((String::new(), EditDenial::TooMany));
            return (applied, denied);
        }
        // A path reported more than once is contradictory: deny every occurrence.
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for p in r
            .changed
            .iter()
            .chain(r.created.iter())
            .map(|f| f.path.as_str())
            .chain(r.deleted.iter().map(String::as_str))
            .chain(r.rejected.iter().map(|(p, _)| p.as_str()))
        {
            *seen.entry(p).or_default() += 1;
        }
        let duplicate = |p: &str| seen.get(p).copied().unwrap_or(0) > 1;
        for (path, kind) in &r.rejected {
            let denial = match validate_path(path) {
                Err(e) => e,
                Ok(()) if policy.is_oracle(path) => EditDenial::OracleProtected,
                Ok(()) => match kind {
                    CopyOutReject::Symlink | CopyOutReject::Special => EditDenial::NotRegularFile,
                    CopyOutReject::Oversize => EditDenial::TooLarge,
                    CopyOutReject::OverCount => EditDenial::TooMany,
                    CopyOutReject::Ignored => EditDenial::DeniedPattern,
                },
            };
            denied.push((path.clone(), denial));
        }
        let changes = r.changed.len() + r.created.len() + r.deleted.len();
        let over_count = changes > MAX_COPY_OUT_ENTRIES;
        let mut entries: Vec<(&str, Reported)> = Vec::new();
        entries.extend(
            r.changed
                .iter()
                .map(|f| (f.path.as_str(), Reported::Changed(f))),
        );
        entries.extend(
            r.created
                .iter()
                .map(|f| (f.path.as_str(), Reported::Created(f))),
        );
        entries.extend(r.deleted.iter().map(|p| (p.as_str(), Reported::Deleted)));
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (path, entry) in entries {
            if duplicate(path) {
                denied.push((path.to_owned(), EditDenial::InvalidPath));
                continue;
            }
            let checked = match entry {
                Reported::Created(_) => self.check_new(policy, path),
                _ => self.check_existing(policy, path),
            };
            if let Err(e) = checked {
                denied.push((path.to_owned(), e));
                continue;
            }
            if over_count {
                denied.push((path.to_owned(), EditDenial::TooMany));
                continue;
            }
            if !contents {
                continue;
            }
            let next = match entry {
                Reported::Deleted => None,
                Reported::Changed(f) | Reported::Created(f) => {
                    match verified(f, policy.limit_file_bytes()) {
                        Ok(bytes) => Some(bytes.to_vec()),
                        Err(e) => {
                            denied.push((path.to_owned(), e));
                            continue;
                        }
                    }
                }
            };
            match self.commit(policy, path, next) {
                Ok(paths) => applied.extend(paths),
                Err(e) => denied.push((path.to_owned(), e)),
            }
        }
        (applied, denied)
    }

    /// [`apply_copy_out`](Self::apply_copy_out) bound to the gate's tree
    /// identity: if the working set changed since the gate staged its tree, the
    /// whole report is refused (a stale copy-out would silently clobber edits).
    pub fn apply_copy_out_checked(
        &mut self,
        policy: &PathPolicy,
        gate_tree_sha256: &str,
        r: &CopyOutReport,
        mode: CopyOutMode,
    ) -> (Vec<String>, Vec<(String, EditDenial)>) {
        if self.manifest().current_sha256 != gate_tree_sha256 {
            return (Vec::new(), vec![(String::new(), EditDenial::HashMismatch)]);
        }
        self.apply_copy_out(policy, r, mode)
    }

    /// Back to base (or removed if task-created). LedgerOnly; hard caps apply.
    pub fn revert_file(&mut self, path: &str) -> Result<(), EditDenial> {
        validate_path(path)?;
        let next = match (self.base.get(path), self.current.contains_key(path)) {
            (Some(b), _) => Some(b.clone()),
            (None, true) => None,
            (None, false) => return Err(EditDenial::NotSelected),
        };
        let caps = PathPolicy::new(BTreeSet::new(), BTreeSet::new(), Vec::new());
        self.commit(&caps, path, next).map(|_| ())
    }

    fn check_existing(&self, policy: &PathPolicy, path: &str) -> Result<(), EditDenial> {
        validate_path(path)?;
        if policy.is_oracle(path) {
            return Err(EditDenial::OracleProtected);
        }
        let current = self.current.get(path).ok_or(EditDenial::NotSelected)?;
        match self.base.get(path) {
            Some(base) => {
                if !policy.selected.contains(path) {
                    return Err(EditDenial::NotSelected);
                }
                if !is_text(base) {
                    return Err(EditDenial::NotUtf8);
                }
            }
            None => {
                if !policy.prefix_allows(path) {
                    return Err(EditDenial::NewPathNotAllowed);
                }
            }
        }
        if !is_text(current) {
            return Err(EditDenial::NotUtf8);
        }
        Ok(())
    }

    fn check_new(&self, policy: &PathPolicy, path: &str) -> Result<(), EditDenial> {
        validate_path(path)?;
        if policy.is_oracle(path) {
            return Err(EditDenial::OracleProtected);
        }
        if self.current.contains_key(path) {
            return Err(EditDenial::NewPathNotAllowed);
        }
        match self.base.get(path) {
            // Re-creating a deleted selected file.
            Some(base) => {
                if !policy.selected.contains(path) {
                    return Err(EditDenial::NotSelected);
                }
                if !is_text(base) {
                    return Err(EditDenial::NotUtf8);
                }
            }
            None => {
                if policy.in_oracle_dir(path)
                    || policy.aliases_oracle_module(path)
                    || (!policy.oracle.is_empty() && test_support_name(path))
                {
                    return Err(EditDenial::OracleProtected);
                }
                if !policy.prefix_allows(path) {
                    return Err(EditDenial::NewPathNotAllowed);
                }
            }
        }
        if self
            .base
            .keys()
            .chain(self.current.keys())
            .any(|e| alias_conflict(e, path))
        {
            return Err(EditDenial::InvalidPath);
        }
        Ok(())
    }

    fn commit(
        &mut self,
        policy: &PathPolicy,
        path: &str,
        next: Option<Vec<u8>>,
    ) -> Result<Vec<String>, EditDenial> {
        let old = self.current.get(path);
        if old == next.as_ref() {
            return Ok(Vec::new());
        }
        let old_len = old.map_or(0, Vec::len);
        let total: usize = self.current.values().map(Vec::len).sum();
        let count = self.current.len() + usize::from(old.is_none()) - usize::from(next.is_none());
        let new_len = next.as_ref().map_or(0, Vec::len);
        if count > policy.limit_files() {
            return Err(EditDenial::TooMany);
        }
        if new_len > policy.limit_file_bytes()
            || total - old_len + new_len > policy.limit_total_bytes()
        {
            return Err(EditDenial::TooLarge);
        }
        match next {
            Some(bytes) => self.current.insert(path.to_owned(), bytes),
            None => self.current.remove(path),
        };
        Ok(vec![path.to_owned()])
    }
}

#[derive(Clone, Copy)]
enum Reported<'a> {
    Changed(&'a ReportedFile),
    Created(&'a ReportedFile),
    Deleted,
}

/// Host-side verification of a reported copy-out file.
fn verified(f: &ReportedFile, limit: usize) -> Result<&[u8], EditDenial> {
    // Bytes are withheld by the supervisor only beyond its caps.
    let bytes = f.bytes.as_deref().ok_or(EditDenial::TooLarge)?;
    if bytes.len() > limit || f.size > limit as u64 {
        return Err(EditDenial::TooLarge);
    }
    if f.sha256.len() != 64
        || !f
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || bytes.len() as u64 != f.size
        || hash(bytes) != f.sha256
    {
        return Err(EditDenial::HashMismatch);
    }
    if !is_text(bytes) {
        return Err(EditDenial::NotUtf8);
    }
    Ok(bytes)
}

fn text_bytes(content: &str, limit: usize) -> Result<Vec<u8>, EditDenial> {
    if content.len() > limit {
        return Err(EditDenial::TooLarge);
    }
    if content.as_bytes().contains(&0) {
        return Err(EditDenial::NotUtf8);
    }
    Ok(content.as_bytes().to_vec())
}

/// `existing` and `path` are distinct but would collide on a case-insensitive
/// filesystem, or one would have to be both a file and a directory.
fn alias_conflict(existing: &str, path: &str) -> bool {
    if existing == path {
        return false;
    }
    let a: Vec<&str> = existing.split('/').collect();
    let b: Vec<&str> = path.split('/').collect();
    for i in 0..a.len().min(b.len()) {
        if a[i] == b[i] {
            if i + 1 == a.len() || i + 1 == b.len() {
                // One name is a directory prefix of the other.
                return true;
            }
            continue;
        }
        return fold(a[i]) == fold(b[i]);
    }
    false
}

/// Strict unified-hunk application: exact context, exact line numbers, ordered
/// non-overlapping hunks, exact counts, `\ No newline at end of file` honoured.
fn apply_unified(current: &str, path: &str, patch: &str) -> Result<String, EditDenial> {
    let bad = EditDenial::PatchContextMismatch;
    let old: Vec<&str> = current.split_inclusive('\n').collect();
    let lines: Vec<&str> = patch.split_inclusive('\n').collect();
    if lines.len() > MAX_PATCH_LINES {
        return Err(EditDenial::TooLarge);
    }
    let mut i = 0;
    // Optional headers naming exactly this path.
    if lines.get(i).is_some_and(|l| l.starts_with("--- ")) {
        let ok = |l: &str, side: &str| {
            let name = l.trim_end_matches('\n').split('\t').next().unwrap_or("");
            name == format!("{side}{path}") || name == path
        };
        if !ok(&lines[i][4..], "a/")
            || !lines
                .get(i + 1)
                .is_some_and(|l| l.starts_with("+++ ") && ok(&l[4..], "b/"))
        {
            return Err(bad);
        }
        i += 2;
    }
    let mut out = String::with_capacity(current.len() + patch.len());
    let (mut cursor, mut delta, mut hunks) = (0usize, 0i64, 0usize);
    let mut eof_seen = false;
    while i < lines.len() {
        let header = lines[i].trim_end_matches('\n');
        if header.is_empty() && i + 1 == lines.len() {
            break;
        }
        let (os, ol, ns, nl) = parse_hunk_header(header).ok_or(bad)?;
        hunks += 1;
        if hunks > MAX_PATCH_HUNKS || eof_seen {
            return Err(bad);
        }
        i += 1;
        let opos = if ol == 0 {
            os
        } else {
            os.checked_sub(1).ok_or(bad)?
        };
        let npos = if nl == 0 {
            ns
        } else {
            ns.checked_sub(1).ok_or(bad)?
        };
        if opos < cursor || opos + ol > old.len() || npos as i64 != opos as i64 + delta {
            return Err(bad);
        }
        let mut old_side: Vec<String> = Vec::new();
        let mut new_side: Vec<String> = Vec::new();
        let mut last: Option<u8> = None;
        while old_side.len() < ol
            || new_side.len() < nl
            || lines.get(i).is_some_and(|l| l.starts_with('\\'))
        {
            let line = *lines.get(i).ok_or(bad)?;
            i += 1;
            let body = line.get(1..).unwrap_or("");
            let text = if body.ends_with('\n') {
                body.to_owned()
            } else {
                format!("{body}\n")
            };
            match line.as_bytes().first() {
                Some(b' ') => {
                    old_side.push(text.clone());
                    new_side.push(text);
                }
                Some(b'-') => old_side.push(text),
                Some(b'+') => new_side.push(text),
                Some(b'\\') => {
                    let strip = |v: &mut Vec<String>| match v.last_mut() {
                        Some(s) if s.ends_with('\n') => {
                            s.pop();
                            Ok(())
                        }
                        _ => Err(bad),
                    };
                    match last {
                        Some(b' ') => {
                            strip(&mut old_side)?;
                            strip(&mut new_side)?;
                        }
                        Some(b'-') => strip(&mut old_side)?,
                        Some(b'+') => strip(&mut new_side)?,
                        _ => return Err(bad),
                    }
                    last = Some(b'\\');
                    continue;
                }
                _ => return Err(bad),
            }
            last = line.as_bytes().first().copied();
            if old_side.len() > ol || new_side.len() > nl {
                return Err(bad);
            }
        }
        // A line without a newline may only be the last line of its side, and
        // only when the hunk reaches the end of the file.
        for side in [&old_side, &new_side] {
            if let Some(k) = side.iter().position(|s| !s.ends_with('\n')) {
                if k + 1 != side.len() || opos + ol != old.len() {
                    return Err(bad);
                }
                eof_seen = true;
            }
        }
        if old[opos..opos + ol]
            .iter()
            .zip(&old_side)
            .any(|(a, b)| *a != b.as_str())
        {
            return Err(bad);
        }
        old[cursor..opos].iter().for_each(|l| out.push_str(l));
        new_side.iter().for_each(|l| out.push_str(l));
        cursor = opos + ol;
        delta += nl as i64 - ol as i64;
    }
    if hunks == 0 {
        return Err(bad);
    }
    old[cursor..].iter().for_each(|l| out.push_str(l));
    Ok(out)
}

/// `@@ -A[,B] +C[,D] @@[ section]` → (A, B, C, D); absent counts are 1.
pub(crate) fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, tail) = rest.split_once(" @@")?;
    if !(tail.is_empty() || tail.starts_with(' ')) {
        return None;
    }
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(usize, usize)> {
        let num = |s: &str| {
            (!s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse().ok())
                .flatten()
        };
        match r.split_once(',') {
            Some((s, l)) => Some((num(s)?, num(l)?)),
            None => Some((num(r)?, 1)),
        }
    };
    let (a, b) = range(old)?;
    let (c, d) = range(new)?;
    Some((a, b, c, d))
}

/// Unverified repository label from `.git` files (design §4.2): `.git/HEAD`, then
/// `refs/heads/<branch>` or the matching `packed-refs` line, through the bounded
/// fd-safe `read_metadata`. No Git command. Any failure → `LabelSource::Unknown`.
pub fn repository_label(
    policy: &SnapshotPolicy,
    root: &Path,
    display_root: String,
    source_id: String,
) -> RepositoryLabel {
    let mut label = RepositoryLabel {
        display_root,
        source_id,
        branch: None,
        head_commit: None,
        label_source: LabelSource::Unknown,
    };
    let read_one = |rel: &str| -> Option<String> {
        let files = read_metadata(policy, root, &[rel.to_owned()]).ok()?;
        String::from_utf8(files.get(rel)?.clone()).ok()
    };
    let Some(head) = read_one(".git/HEAD") else {
        return label;
    };
    let head = head.strip_suffix('\n').unwrap_or(&head);
    if let Some(reference) = head.strip_prefix("ref: refs/heads/") {
        if !branch_name_ok(reference) {
            return label;
        }
        label.branch = Some(reference.to_owned());
        label.head_commit = read_one(&format!(".git/refs/heads/{reference}"))
            .map(|s| s.trim_end_matches('\n').to_owned())
            .filter(|h| commit_ok(h))
            .or_else(|| {
                read_one(".git/packed-refs")?.lines().find_map(|l| {
                    let (h, r) = l.split_once(' ')?;
                    (r == format!("refs/heads/{reference}") && commit_ok(h)).then(|| h.to_owned())
                })
            });
    } else if commit_ok(head) {
        label.head_commit = Some(head.to_owned());
    } else {
        return label;
    }
    label.label_source = LabelSource::GitFilesUnverified;
    label
}

fn commit_ok(h: &str) -> bool {
    (h.len() == 40 || h.len() == 64)
        && h.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn branch_name_ok(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= 128
        && b.split('/').count() <= 4
        && b.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c))
        && !b.contains("..")
        && !b.ends_with(".lock")
        && b.split('/')
            .all(|c| !c.is_empty() && !c.starts_with('.') && !c.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKER: &str = "SYNTHETIC_PRIVATE_MARKER_7f3a91";

    fn files(entries: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(p, b)| ((*p).to_owned(), b.to_vec()))
            .collect()
    }
    fn working(entries: &[(&str, &[u8])]) -> WorkingSet {
        let f = files(entries);
        WorkingSet::from_parts(f.clone(), f).unwrap()
    }
    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }
    fn policy_for(ws: &WorkingSet, oracle: &[&str], prefixes: &[&str]) -> PathPolicy {
        PathPolicy::new(
            ws.base_files().keys().cloned().collect(),
            set(oracle),
            prefixes.iter().map(|s| (*s).to_owned()).collect(),
        )
    }
    fn create(path: &str, content: &str) -> ProposedEdit {
        ProposedEdit::Create {
            path: path.into(),
            content: content.into(),
        }
    }
    fn replace(path: &str, content: &str) -> ProposedEdit {
        ProposedEdit::Replace {
            path: path.into(),
            content: content.into(),
        }
    }
    fn patch(path: &str, hunks: &str) -> ProposedEdit {
        ProposedEdit::Patch {
            path: path.into(),
            unified_hunks: hunks.into(),
        }
    }
    fn reported(path: &str, bytes: &[u8]) -> ReportedFile {
        ReportedFile {
            path: path.into(),
            sha256: hash(bytes),
            size: bytes.len() as u64,
            bytes: Some(bytes.to_vec()),
        }
    }

    #[test]
    fn task_workspace_path_policy_matrix() {
        let long = "x".repeat(MAX_PATH_BYTES + 1);
        let invalid: Vec<&str> = vec![
            "",
            "/etc/passwd",
            "/abs.py",
            "../private/marker",
            "a/../b.py",
            "./a.py",
            "a/./b.py",
            "a//b.py",
            "a/",
            "/",
            ".",
            "..",
            "a\\b.py",
            "..\\private\\marker",
            "a\0b.py",
            "a\nb.py",
            "a\rb.py",
            "a\tb.py",
            "a\u{7f}b.py",
            "a\u{85}b.py",
            "evil\u{202e}yp.py",
            "a\u{200b}b.py",
            "\u{feff}a.py",
            "a\u{2066}b.py",
            "C:/x.py",
            "a:b.py",
            "a*b.py",
            "a?b.py",
            "a|b.py",
            "a\"b.py",
            "a<b.py",
            "a>b.py",
            "file.py.",
            "file.py ",
            "dir./x.py",
            &long,
            "1/2/3/4/5/6/7/8/9.py",
        ];
        for p in &invalid {
            assert_eq!(validate_path(p), Err(EditDenial::InvalidPath), "{p:?}");
        }
        let denied = [
            ".git/config",
            ".GIT/config",
            ".Git/HEAD",
            "src/.git/hooks/pre-commit",
            ".hg/store",
            ".svn/entries",
            ".env",
            ".ENV",
            "app/.Env",
            ".env.local",
            ".Env.Production",
            ".envrc",
            "id_rsa",
            "ID_RSA.pub",
            "keys/id_ed25519",
            "certs/server.pem",
            "x.KEY",
            "x.\u{212a}ey",
            "a.p12",
            "a.PFX",
            "vault.kdbx",
            "node_modules/left-pad/index.js",
            "NODE_MODULES/x.js",
            "src/__pycache__/m.cpython-39.pyc",
            ".pytest_cache/v",
            ".pai-tmp-0011",
            ".PAI-anything",
            "GIT~1/config",
            "git~12/x",
            ".g\u{131}t/config",
            ".ssh/config",
            ".netrc",
            ".npmrc",
            "con",
            "CON.txt",
            "src/nul.py",
            "com1",
            "LPT9.log",
            "com\u{b9}",
            "aux.tar.gz",
        ];
        for p in &denied {
            assert_eq!(validate_path(p), Err(EditDenial::DeniedPattern), "{p:?}");
        }
        let depth8 = "a/b/c/d/e/f/g/h.py";
        let max = "x".repeat(MAX_PATH_BYTES);
        let allowed = [
            "src/main.py",
            "README.md",
            depth8,
            "tests/test_x.py",
            ".github/workflows/ci.yml",
            ".gitignore",
            "envfile.py",
            "environment.py",
            "my file.py",
            "keys.py",
            "src/key_utils.py",
            "console.py",
            "com10.py",
            "\u{73af}\u{5883}.py",
            &max,
        ];
        for p in &allowed {
            assert_eq!(validate_path(p), Ok(()), "{p:?}");
        }
        // Allowed-new prefixes: component-wise, no sibling-prefix tricks.
        let ws = working(&[("src/main.py", b"x = 1\n")]);
        let ok_policy = policy_for(&ws, &[], &["src/"]);
        let mut w = ws.clone();
        assert_eq!(
            w.apply_edit(&ok_policy, &create("src/new.py", "y\n")),
            Ok(vec!["src/new.py".into()])
        );
        for p in ["src-evil/new.py", "srcx/a.py", "lib/src/a.py", "new.py"] {
            assert_eq!(
                ws.clone().apply_edit(&ok_policy, &create(p, "y\n")),
                Err(EditDenial::NewPathNotAllowed),
                "{p}"
            );
        }
        for bad_prefixes in [
            vec!["src"],
            vec![""],
            vec!["/"],
            vec!["../"],
            vec!["./src/"],
            vec!["src//"],
            vec![".git/"],
            vec!["/src/"],
            vec!["a/", "b/", "c/", "d/", "e/", "f/", "g/", "h/", "src/"],
        ] {
            let p = policy_for(&ws, &[], &bad_prefixes);
            assert_eq!(
                ws.clone().apply_edit(&p, &create("src/new.py", "y\n")),
                Err(EditDenial::NewPathNotAllowed),
                "{bad_prefixes:?}"
            );
            assert_eq!(
                p.check_selection(),
                Err((String::new(), EditDenial::NewPathNotAllowed))
            );
        }
        // Case variants and file/dir conflicts never alias an existing name.
        let p = policy_for(&ws, &[], &["src/", "SRC/", "Src/"]);
        for alias in [
            "SRC/main.py",
            "src/Main.py",
            "Src/MAIN.PY",
            "src/main.py/x.py",
            "SRC/other.py",
        ] {
            assert_eq!(
                ws.clone().apply_edit(&p, &create(alias, "z\n")),
                Err(EditDenial::InvalidPath),
                "{alias}"
            );
        }
        // Selection checks (HA31 shape): refused before any read; no content.
        let sel = |paths: &[&str]| PathPolicy::new(set(paths), BTreeSet::new(), vec![]);
        assert_eq!(
            sel(&["main.py", "../private/marker"]).check_selection(),
            Err(("../private/marker".into(), EditDenial::InvalidPath))
        );
        assert_eq!(
            sel(&["/etc/shadow"]).check_selection(),
            Err(("/etc/shadow".into(), EditDenial::InvalidPath))
        );
        assert_eq!(
            sel(&["main.py", ".env"]).check_selection(),
            Err((".env".into(), EditDenial::DeniedPattern))
        );
        assert_eq!(
            sel(&["a.py", "A.py"]).check_selection(),
            Err(("a.py".into(), EditDenial::InvalidPath))
        );
        let many: Vec<String> = (0..33).map(|i| format!("f{i}.py")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            sel(&many).check_selection(),
            Err((String::new(), EditDenial::TooMany))
        );
        assert_eq!(sel(&["main.py", "src/util.py"]).check_selection(), Ok(()));
        // Denials are content-free.
        for d in [
            EditDenial::InvalidPath,
            EditDenial::OracleProtected,
            EditDenial::HashMismatch,
        ] {
            assert!(!format!("{d} {d:?}").contains(MARKER));
        }
    }

    #[test]
    fn task_workspace_edit_rules_bounds_and_atomicity() {
        let base = working(&[
            ("src/calc.py", b"def add(a, b):\n    return a - b\n"),
            ("src/util.py", b"X = 1\n"),
            (
                "tests/test_calc.py",
                b"from src.calc import add\nassert add(1, 2) == 3\n",
            ),
            ("data/blob.bin", b"\xff\xfe\x00binary"),
        ]);
        let policy = policy_for(&base, &["tests/test_calc.py"], &["src/", "tests/"]);
        let mut ws = base.clone();
        let before = ws.manifest();
        // Oracle: never editable, deletable, re-creatable or case-shadowed.
        for e in [
            replace("tests/test_calc.py", "assert True\n"),
            patch(
                "tests/test_calc.py",
                "@@ -2 +2 @@\n-assert add(1, 2) == 3\n+assert True\n",
            ),
            ProposedEdit::Delete {
                path: "tests/test_calc.py".into(),
            },
            create("Tests/test_calc.py", "assert True\n"),
            create("tests/TEST_CALC.py", "assert True\n"),
        ] {
            assert_eq!(
                ws.apply_edit(&policy, &e),
                Err(EditDenial::OracleProtected),
                "{e:?}"
            );
        }
        // New files that would join the oracle's process or directory.
        let shouting = PathPolicy {
            allowed_new_prefixes: vec!["TESTS/".into(), "src/".into()],
            ..policy.clone()
        };
        assert_eq!(
            ws.apply_edit(&shouting, &create("TESTS/new.py", "x\n")),
            Err(EditDenial::OracleProtected)
        );
        for p in [
            "tests/helpers2.py",
            "tests/sub/x.py",
            "src/test_extra.py",
            "src/extra_test.py",
            "src/conftest.py",
            "src/sitecustomize.py",
            "src/usercustomize.py",
            "src/evil.pth",
            "src/pytest.ini",
        ] {
            assert_eq!(
                ws.apply_edit(&policy, &create(p, "import os\nos._exit(0)\n")),
                Err(EditDenial::OracleProtected),
                "{p}"
            );
        }
        assert_eq!(
            ws.apply_edit(&policy, &replace("lib/other.py", "x\n")),
            Err(EditDenial::NotSelected)
        );
        assert_eq!(
            ws.apply_edit(&policy, &create("src/calc.py", "x\n")),
            Err(EditDenial::NewPathNotAllowed)
        );
        assert_eq!(
            ws.apply_edit(&policy, &create("lib/x.py", "x\n")),
            Err(EditDenial::NewPathNotAllowed)
        );
        assert_eq!(
            ws.apply_edit(&policy, &replace("src/calc.py", "a\0b")),
            Err(EditDenial::NotUtf8)
        );
        assert_eq!(
            ws.apply_edit(&policy, &replace("data/blob.bin", "text")),
            Err(EditDenial::NotUtf8)
        );
        assert_eq!(
            ws.apply_edit(
                &policy,
                &ProposedEdit::Delete {
                    path: "data/blob.bin".into()
                }
            ),
            Err(EditDenial::NotUtf8)
        );
        let big = "x".repeat(HARD_MAX_FILE_BYTES + 1);
        assert_eq!(
            ws.apply_edit(&policy, &replace("src/calc.py", &big)),
            Err(EditDenial::TooLarge)
        );
        // Values above the hard caps are clamped, never raised.
        let mut loose = policy.clone();
        (loose.max_file_bytes, loose.max_total_bytes, loose.max_files) =
            (usize::MAX, usize::MAX, usize::MAX);
        assert_eq!(
            ws.apply_edit(&loose, &replace("src/calc.py", &big)),
            Err(EditDenial::TooLarge)
        );
        let mut tight = policy.clone();
        tight.max_total_bytes = 200;
        assert_eq!(
            ws.apply_edit(&tight, &replace("src/util.py", &"y".repeat(150))),
            Err(EditDenial::TooLarge)
        );
        tight.max_files = 4;
        assert_eq!(
            ws.apply_edit(&tight, &create("src/n.py", "n\n")),
            Err(EditDenial::TooMany)
        );
        assert_eq!(
            ws.manifest(),
            before,
            "denied edits must not change anything"
        );
        // Allowed edits.
        assert_eq!(
            ws.apply_edit(
                &policy,
                &replace("src/calc.py", "def add(a, b):\n    return a + b\n")
            ),
            Ok(vec!["src/calc.py".into()])
        );
        assert_eq!(
            ws.apply_edit(
                &policy,
                &replace("src/calc.py", "def add(a, b):\n    return a + b\n")
            ),
            Ok(vec![])
        );
        assert_eq!(
            ws.apply_edit(&policy, &create("src/new.py", "N = 2\n")),
            Ok(vec!["src/new.py".into()])
        );
        assert_eq!(
            ws.apply_edit(&policy, &replace("src/new.py", "N = 3\n")),
            Ok(vec!["src/new.py".into()])
        );
        assert_eq!(
            ws.apply_edit(
                &policy,
                &ProposedEdit::Delete {
                    path: "src/util.py".into()
                }
            ),
            Ok(vec!["src/util.py".into()])
        );
        assert_eq!(
            ws.apply_edit(&policy, &create("src/util.py", "X = 2\n")),
            Ok(vec!["src/util.py".into()])
        );
        assert_eq!(
            ws.changed_paths(),
            vec!["src/calc.py", "src/new.py", "src/util.py"]
        );
        assert_eq!(
            ws.files()["tests/test_calc.py"],
            base.files()["tests/test_calc.py"]
        );
        // Revert: base content back, created file removed, unknown refused.
        ws.revert_file("src/calc.py").unwrap();
        ws.revert_file("src/new.py").unwrap();
        ws.revert_file("src/util.py").unwrap();
        assert_eq!(ws.revert_file("nope.py"), Err(EditDenial::NotSelected));
        assert_eq!(ws.revert_file("../x"), Err(EditDenial::InvalidPath));
        assert_eq!(ws.manifest(), before);
        assert!(ws.changed_paths().is_empty());
        // Debug never prints content.
        assert!(!format!("{ws:?}").contains("return a"));
        // Strict JSON for proposals.
        assert!(serde_json::from_str::<ProposedEdit>(
            r#"{"kind":"replace","path":"a","content":"b","auto_accept":true}"#
        )
        .is_err());
        assert!(
            serde_json::from_str::<ProposedEdit>(r#"{"kind":"delete","path":"src/util.py"}"#)
                .is_ok()
        );
    }

    #[test]
    fn task_workspace_patch_context_mismatch_rejected() {
        let ws = working(&[
            ("f.py", b"a\nb\nc\nd\ne\n"),
            ("noeol.py", b"x\ny"),
            ("crlf.py", b"one\r\ntwo\r\n"),
        ]);
        let policy = policy_for(&ws, &[], &[]);
        let run = |path: &str, hunks: &str| {
            let mut w = ws.clone();
            w.apply_edit(&policy, &patch(path, hunks))
                .map(|_| String::from_utf8(w.files()[path].clone()).unwrap())
        };
        assert_eq!(
            run("f.py", "@@ -2,2 +2,2 @@\n b\n-c\n+C\n").unwrap(),
            "a\nb\nC\nd\ne\n"
        );
        assert_eq!(
            run("f.py", "--- a/f.py\n+++ b/f.py\n@@ -3 +3 @@\n-c\n+C\n").unwrap(),
            "a\nb\nC\nd\ne\n"
        );
        assert_eq!(
            run(
                "f.py",
                "--- f.py\t2026-01-01\n+++ f.py\t2026-01-01\n@@ -3 +3 @@\n-c\n+C\n"
            )
            .unwrap(),
            "a\nb\nC\nd\ne\n"
        );
        assert_eq!(
            run("f.py", "@@ -0,0 +1 @@\n+first\n").unwrap(),
            "first\na\nb\nc\nd\ne\n"
        );
        assert_eq!(
            run("f.py", "@@ -5,0 +6 @@\n+last\n").unwrap(),
            "a\nb\nc\nd\ne\nlast\n"
        );
        assert_eq!(
            run("f.py", "@@ -1,5 +0,0 @@\n-a\n-b\n-c\n-d\n-e\n").unwrap(),
            ""
        );
        assert_eq!(
            run("f.py", "@@ -1 +1 @@\n-a\n+A\n@@ -5 +5 @@\n-e\n+E\n").unwrap(),
            "A\nb\nc\nd\nE\n"
        );
        assert_eq!(
            run(
                "noeol.py",
                "@@ -2 +2 @@\n-y\n\\ No newline at end of file\n+Y\n\\ No newline at end of file\n"
            )
            .unwrap(),
            "x\nY"
        );
        assert_eq!(
            run(
                "noeol.py",
                "@@ -2 +2 @@\n-y\n\\ No newline at end of file\n+y\n"
            )
            .unwrap(),
            "x\ny\n"
        );
        assert_eq!(
            run("crlf.py", "@@ -2 +2 @@\n-two\r\n+TWO\r\n").unwrap(),
            "one\r\nTWO\r\n"
        );
        let mismatch = [
            ("f.py", ""),
            ("f.py", "garbage\n@@ -3 +3 @@\n-c\n+C\n"),
            (
                "f.py",
                "--- a/other.py\n+++ b/other.py\n@@ -3 +3 @@\n-c\n+C\n",
            ),
            ("f.py", "--- a/f.py\n+++ b/g.py\n@@ -3 +3 @@\n-c\n+C\n"),
            ("f.py", "@@ -2,2 +2,2 @@\n x\n-c\n+C\n"),
            ("f.py", "@@ -3,2 +3,2 @@\n b\n-c\n+C\n"),
            ("f.py", "@@ -4 +4 @@\n-c\n+C\n"),
            ("f.py", "@@ -3 +4 @@\n-c\n+C\n"),
            ("f.py", "@@ -3,2 +3 @@\n-c\n+C\n"),
            ("f.py", "@@ -3 +3,2 @@\n-c\n+C\n"),
            ("f.py", "@@ -3 +3 @@\n-c\n+C\n+extra\n"),
            ("f.py", "@@ -3 +3 @@\n-c\n"),
            ("f.py", "@@ -2,2 +2,2 @@\nb\n-c\n+C\n"),
            ("f.py", "@@ -4 +4 @@\n-d\n+D\n@@ -3 +3 @@\n-c\n+C\n"),
            (
                "f.py",
                "@@ -2,2 +2,2 @@\n-b\n-c\n+B\n+C\n@@ -3 +3 @@\n-c\n+C\n",
            ),
            ("f.py", "@@ -3 +3 @@\n-c\n+C\n trailing garbage\n"),
            (
                "f.py",
                "@@ -3 +3 @@\n-c\n\\ No newline at end of file\n+C\n",
            ),
            (
                "f.py",
                "@@ -3 +3 @@\n-c\n+C\n\\ No newline at end of file\n",
            ),
            ("f.py", "@@ -9 +9 @@\n-z\n+Z\n"),
            ("f.py", "@@ -a +1 @@\n-a\n+A\n"),
            ("f.py", "@@ -1 +1 @@junk\n-a\n+A\n"),
            ("noeol.py", "@@ -2 +2 @@\n-y\n+Y\n"),
            ("crlf.py", "@@ -2 +2 @@\n-two\n+TWO\n"),
        ];
        for (path, hunks) in mismatch {
            assert_eq!(
                run(path, hunks),
                Err(EditDenial::PatchContextMismatch),
                "{hunks:?}"
            );
        }
        // Policy is checked before the patch is even parsed.
        let oracle = policy_for(&ws, &["f.py"], &[]);
        assert_eq!(
            ws.clone()
                .apply_edit(&oracle, &patch("f.py", "@@ -3 +3 @@\n-c\n+C\n")),
            Err(EditDenial::OracleProtected)
        );
    }

    #[test]
    fn task_workspace_copy_out_matrix() {
        let base = working(&[
            ("src/calc.py", b"def f():\n    return 1\n"),
            ("src/util.py", b"U = 1\n"),
            ("src/other.py", b"O = 1\n"),
            (
                "tests/test_calc.py",
                b"from tests.helpers import check\nimport src.calc\ncheck()\n",
            ),
            ("tests/helpers.py", b"def check():\n    assert True\n"),
            ("tests/fixture.json", b"{}\n"),
        ]);
        let (policy, derivation) = PathPolicy::for_selection(
            base.base_files(),
            &set(&["tests/test_calc.py"]),
            vec!["src/".into(), "tests/".into()],
        );
        assert!(policy.oracle.contains("tests/helpers.py"), "{derivation:?}");
        let fixed = b"def f():\n    return 2\n".as_slice();
        let mut bad_sha = reported("src/other.py", b"O = 2\n");
        bad_sha.sha256 = hash(b"something else");
        let mut bad_size = reported("src/calc.py", fixed);
        bad_size.size += 1;
        let mut upper = reported("src/calc.py", fixed);
        upper.sha256 = upper.sha256.to_uppercase();
        let mut withheld = reported("src/calc.py", fixed);
        withheld.bytes = None;
        let report = CopyOutReport {
            changed: vec![
                reported("src/calc.py", fixed),
                bad_sha,
                reported("tests/test_calc.py", b"assert True\n"),
                reported("lib/notselected.py", b"x\n"),
            ],
            created: vec![
                reported("src/new.py", b"N = 1\n"),
                reported("lib/evil.py", b"x\n"),
                reported("tests/test_extra.py", b"import os\nos._exit(0)\n"),
                reported(".git/hooks/post-checkout", b"#!/bin/sh\n"),
                reported("../outside.py", MARKER.as_bytes()),
                reported("/abs.py", MARKER.as_bytes()),
                reported("src/blob.pyc", b"\x00\xffbinary"),
                reported("src/big.py", &vec![b'x'; HARD_MAX_FILE_BYTES + 1]),
                reported("src/dup.py", b"d\n"),
            ],
            deleted: vec![
                "src/util.py".into(),
                "tests/helpers.py".into(),
                "src/dup.py".into(),
            ],
            rejected: vec![
                ("src/link.py".into(), CopyOutReject::Symlink),
                ("src/fifo".into(), CopyOutReject::Special),
                ("src/huge.py".into(), CopyOutReject::Oversize),
                ("src/many.py".into(), CopyOutReject::OverCount),
                (
                    "src/__pycache__/calc.cpython-39.pyc".into(),
                    CopyOutReject::Ignored,
                ),
                ("tests/fixture.json".into(), CopyOutReject::Symlink),
            ],
        };
        let mut ws = base.clone();
        let (applied, denied) = ws.apply_copy_out(&policy, &report, CopyOutMode::Contents);
        let denied: BTreeMap<String, EditDenial> = denied.into_iter().collect();
        assert_eq!(
            applied,
            vec!["src/calc.py", "src/new.py", "src/util.py"],
            "{denied:?}"
        );
        let expect = [
            ("src/other.py", EditDenial::HashMismatch),
            ("tests/fixture.json", EditDenial::OracleProtected),
            ("tests/test_calc.py", EditDenial::OracleProtected),
            ("tests/helpers.py", EditDenial::OracleProtected),
            ("lib/notselected.py", EditDenial::NotSelected),
            ("lib/evil.py", EditDenial::NewPathNotAllowed),
            ("tests/test_extra.py", EditDenial::OracleProtected),
            (".git/hooks/post-checkout", EditDenial::DeniedPattern),
            ("../outside.py", EditDenial::InvalidPath),
            ("/abs.py", EditDenial::InvalidPath),
            ("src/blob.pyc", EditDenial::NotUtf8),
            ("src/big.py", EditDenial::TooLarge),
            ("src/dup.py", EditDenial::InvalidPath),
            ("src/link.py", EditDenial::NotRegularFile),
            ("src/fifo", EditDenial::NotRegularFile),
            ("src/huge.py", EditDenial::TooLarge),
            ("src/many.py", EditDenial::TooMany),
            (
                "src/__pycache__/calc.cpython-39.pyc",
                EditDenial::DeniedPattern,
            ),
        ];
        for (path, denial) in expect {
            assert_eq!(denied.get(path), Some(&denial), "{path}: {denied:?}");
        }
        assert_eq!(ws.files()["src/calc.py"], fixed);
        assert!(!ws.files().contains_key("src/util.py"));
        assert_eq!(ws.files()["src/other.py"], base.files()["src/other.py"]);
        assert_eq!(
            ws.files()["tests/test_calc.py"],
            base.files()["tests/test_calc.py"]
        );
        assert_eq!(
            ws.files()["tests/helpers.py"],
            base.files()["tests/helpers.py"]
        );
        assert!(
            !ws.files().contains_key("../outside.py") && !ws.files().contains_key("lib/evil.py")
        );
        // Size / digest / withheld-bytes cases one at a time.
        for (f, denial) in [
            (bad_size, EditDenial::HashMismatch),
            (upper, EditDenial::HashMismatch),
            (withheld, EditDenial::TooLarge),
        ] {
            let mut w = base.clone();
            let r = CopyOutReport {
                changed: vec![f],
                ..CopyOutReport::default()
            };
            assert_eq!(
                w.apply_copy_out(&policy, &r, CopyOutMode::Contents),
                (vec![], vec![("src/calc.py".to_owned(), denial)])
            );
            assert_eq!(w, base);
        }
        // hashes_only / off never change the working set but still surface
        // oracle / policy denials.
        for mode in [CopyOutMode::HashesOnly, CopyOutMode::Off] {
            let mut w = base.clone();
            let (applied, denied) = w.apply_copy_out(&policy, &report, mode);
            assert!(applied.is_empty());
            assert!(denied.contains(&("tests/test_calc.py".into(), EditDenial::OracleProtected)));
            assert_eq!(w, base);
        }
        // Stale gate tree: whole report refused.
        let mut w = base.clone();
        assert_eq!(
            w.apply_copy_out_checked(&policy, &"0".repeat(64), &report, CopyOutMode::Contents),
            (vec![], vec![(String::new(), EditDenial::HashMismatch)])
        );
        let tree = base.manifest().current_sha256;
        let ok = CopyOutReport {
            changed: vec![reported("src/calc.py", fixed)],
            ..CopyOutReport::default()
        };
        assert_eq!(
            w.apply_copy_out_checked(&policy, &tree, &ok, CopyOutMode::Contents)
                .0,
            vec!["src/calc.py"]
        );
        // Over-count: every entry denied.
        let flood = CopyOutReport {
            created: (0..(MAX_COPY_OUT_ENTRIES + 1))
                .map(|i| reported(&format!("src/f{i}.py"), b"x\n"))
                .collect(),
            ..CopyOutReport::default()
        };
        let mut w = base.clone();
        let (applied, denied) = w.apply_copy_out(&policy, &flood, CopyOutMode::Contents);
        assert!(applied.is_empty() && denied.iter().all(|(_, d)| *d == EditDenial::TooMany));
        assert_eq!(w, base);
        // Marker bytes from rejected entries never appear in denials.
        assert!(!format!("{denied:?}").contains(MARKER));
    }

    #[test]
    fn task_workspace_oracle_derivation_includes_helpers() {
        let src = files(&[
            ("calc.py", b"def add(a, b): return a + b\n"),
            ("invoice.py", b"import calc\n"),
            ("fixtures.py", b"def load(): return []\n"),
            ("expected.json", b"[3]\n"),
            ("conftest.py", b"\n"),
            ("pkg/__init__.py", b"\n"),
            ("pkg/testing_utils.py", b"X = 1\n"),
            ("pkg/core.py", b"Y = 1\n"),
            ("src/app.py", b"Z = 1\n"),
            (
                "test_root.py",
                b"import calc, invoice\nfrom fixtures import (\n    load,\n)\nfrom pkg.testing_utils import X\nfrom pkg import core\nDATA = open('expected.json').read()\n",
            ),
            ("tests/test_calc.py", b"from . import common\nfrom tests.helpers import approx\n"),
            ("tests/common.py", b"\n"),
            ("tests/helpers.py", b"import importlib\nm = importlib.import_module(\"pkg.core\")\n"),
            ("tests/data/vectors.json", b"{}\n"),
        ]);
        let d = derive_oracle_set(&src, &set(&["test_root.py", "tests/test_calc.py"]));
        let reason = |p: &str| d.reasons.get(p).cloned();
        assert_eq!(reason("test_root.py"), Some(OracleReason::Declared));
        assert_eq!(
            reason("tests/common.py"),
            Some(OracleReason::InOracleDirectory)
        );
        assert_eq!(
            reason("tests/helpers.py"),
            Some(OracleReason::InOracleDirectory)
        );
        assert_eq!(
            reason("tests/data/vectors.json"),
            Some(OracleReason::InOracleDirectory)
        );
        assert_eq!(reason("conftest.py"), Some(OracleReason::ConftestAncestor));
        assert_eq!(
            reason("fixtures.py"),
            Some(OracleReason::ImportedHelper {
                by: "test_root.py".into()
            })
        );
        assert_eq!(
            reason("pkg/testing_utils.py"),
            Some(OracleReason::ImportedHelper {
                by: "test_root.py".into()
            })
        );
        assert_eq!(
            reason("expected.json"),
            Some(OracleReason::ReferencedData {
                by: "test_root.py".into()
            })
        );
        // Implementation modules under test stay editable.
        for p in [
            "calc.py",
            "invoice.py",
            "pkg/core.py",
            "pkg/__init__.py",
            "src/app.py",
        ] {
            assert!(!d.oracle.contains(p), "{p} must stay editable: {d:?}");
        }
        let (policy, _) = PathPolicy::for_selection(
            &src,
            &set(&["test_root.py", "tests/test_calc.py"]),
            vec!["pkg/".into()],
        );
        let mut ws = WorkingSet::from_parts(src.clone(), src.clone()).unwrap();
        assert_eq!(
            ws.apply_edit(&policy, &replace("fixtures.py", "def load(): return [3]\n")),
            Err(EditDenial::OracleProtected)
        );
        assert_eq!(
            ws.apply_edit(&policy, &replace("expected.json", "[]\n")),
            Err(EditDenial::OracleProtected)
        );
        assert_eq!(
            ws.apply_edit(&policy, &replace("calc.py", "def add(a, b): return 3\n")),
            Ok(vec!["calc.py".into()])
        );
    }

    #[test]
    fn task_workspace_python_module_alias_of_protected_module_refused() {
        // R3 C9 d7 (L1): with a user-confirmed prefix equal to the stem of a
        // protected ROOT-level helper, a new `checks_support/__init__.py` was
        // accepted and Python imported that package instead of the protected
        // `checks_support.py` (the oracle verdict flipped 1 -> 0).
        let base = working(&[
            ("temperature.py", b"def c_to_k(c):\n    return c + 273\n"),
            ("tests/__init__.py", b"\n"),
            (
                "tests/test_support_check.py",
                b"import checks_support\nimport temperature\n",
            ),
            ("checks_support.py", b"PLACES = 2\n"),
            ("lib/__init__.py", b"\n"),
            ("lib/core.py", b"C = 1\n"),
            ("lib/support/__init__.py", b"S = 1\n"),
            ("lib/helpers/fixture_data.py", b"D = 1\n"),
        ]);
        let oracle = [
            "tests/__init__.py",
            "tests/test_support_check.py",
            "checks_support.py",
            "lib/support/__init__.py",
            "lib/helpers/fixture_data.py",
        ];
        let prefixes = [
            "src/",
            "checks_support/",
            "Checks_Support/",
            "CHECKS_SUPPORT/",
            "checks_supportx/",
            "lib/",
        ];
        let policy = policy_for(&base, &oracle, &prefixes);
        assert_eq!(policy.check_selection(), Ok(()));
        let mut ws = base.clone();
        let before = ws.manifest();
        for p in [
            // d7 itself: the regular package beats the protected module.
            "checks_support/__init__.py",
            // Any entry below `checks_support/` (namespace portion, nested
            // package, data) and case variants of the directory.
            "checks_support/x.py",
            "checks_support/sub/__init__.py",
            "checks_support/notes.txt",
            "Checks_Support/__init__.py",
            "CHECKS_SUPPORT/__INIT__.PY",
            // Other import sources for the module name `checks_support`.
            "checks_support.pyc",
            "checks_support.pyo",
            "checks_support.pyw",
            "checks_support.pyd",
            "checks_support.so",
            "checks_support.cpython-312-x86_64-linux-gnu.so",
            "checks_support.cp312-win_amd64.pyd",
            "Checks_Support.PYC",
            // Symmetric: a protected package `lib/support/__init__.py`, and a
            // protected module below `lib/helpers/`, refuse a sibling module.
            "lib/support.py",
            "lib/Support.py",
            "lib/support.pyc",
            "lib/SUPPORT.cpython-312-x86_64-linux-gnu.so",
            "lib/helpers.py",
            "lib/Helpers.pyw",
        ] {
            assert_eq!(
                ws.apply_edit(&policy, &create(p, "PLACES = -1\n")),
                Err(EditDenial::OracleProtected),
                "{p}"
            );
        }
        assert_eq!(
            ws.manifest(),
            before,
            "denied aliases must not change anything"
        );
        // Rename (delete + create) into the shadowing package is refused at the
        // create step; the protected module itself never moves.
        let mut w = base.clone();
        assert_eq!(
            w.apply_edit(
                &policy,
                &ProposedEdit::Delete {
                    path: "lib/core.py".into()
                }
            ),
            Ok(vec!["lib/core.py".into()])
        );
        assert_eq!(
            w.apply_edit(&policy, &create("checks_support/__init__.py", "C = 1\n")),
            Err(EditDenial::OracleProtected)
        );
        assert_eq!(
            w.apply_edit(
                &policy,
                &ProposedEdit::Delete {
                    path: "checks_support.py".into()
                }
            ),
            Err(EditDenial::OracleProtected)
        );
        // Unrelated names stay allowed: the same stem in ANOTHER directory
        // imports under another name (`src.checks_support`, R3 d6/c5), and
        // different-stem or non-module siblings are no import source.
        for p in [
            "src/checks_support.py",
            "src/checks_support/__init__.py",
            "checks_supportx/__init__.py",
            "lib/supports.py",
            "lib/support_extra.py",
            "lib/support.md",
            "lib/new.py",
        ] {
            assert_eq!(
                base.clone().apply_edit(&policy, &create(p, "X = 1\n")),
                Ok(vec![p.to_owned()]),
                "{p}"
            );
        }
        // Sandbox copy-out: the same rule for created entries, including an
        // in-sandbox rename of the protected module into a package, in every mode.
        let report = CopyOutReport {
            created: vec![
                reported("checks_support/__init__.py", b"PLACES = -1\n"),
                reported("Checks_Support/x.py", b"X = 1\n"),
                reported("checks_support.pyw", b"PLACES = -1\n"),
                reported("lib/support.py", b"S = -1\n"),
                reported("lib/helpers.py", b"D = -1\n"),
                reported("src/checks_support.py", b"PLACES = -1\n"),
            ],
            deleted: vec!["checks_support.py".into()],
            ..CopyOutReport::default()
        };
        let mut w = base.clone();
        let (applied, denied) = w.apply_copy_out(&policy, &report, CopyOutMode::Contents);
        assert_eq!(applied, vec!["src/checks_support.py"], "{denied:?}");
        let denied: BTreeMap<String, EditDenial> = denied.into_iter().collect();
        let expect: BTreeMap<String, EditDenial> = [
            "checks_support/__init__.py",
            "Checks_Support/x.py",
            "checks_support.pyw",
            "lib/support.py",
            "lib/helpers.py",
            "checks_support.py",
        ]
        .iter()
        .map(|p| ((*p).to_owned(), EditDenial::OracleProtected))
        .collect();
        assert_eq!(denied, expect);
        assert_eq!(
            w.files()["checks_support.py"],
            base.files()["checks_support.py"]
        );
        for mode in [CopyOutMode::HashesOnly, CopyOutMode::Off] {
            let mut w = base.clone();
            let (applied, denied) = w.apply_copy_out(&policy, &report, mode);
            assert!(applied.is_empty());
            assert!(denied.contains(&(
                "checks_support/__init__.py".into(),
                EditDenial::OracleProtected
            )));
            assert_eq!(w, base);
        }
        // Same rule over a DERIVED root-level helper (`for_selection`).
        let src = files(&[
            ("calc.py", b"def add(a, b): return a + b\n"),
            (
                "tests/test_calc.py",
                b"import calc\nimport fixture_support\n",
            ),
            ("fixture_support.py", b"TOL = 0\n"),
        ]);
        let (derived, d) = PathPolicy::for_selection(
            &src,
            &set(&["tests/test_calc.py"]),
            vec!["fixture_support/".into(), "src/".into()],
        );
        assert!(derived.oracle.contains("fixture_support.py"), "{d:?}");
        let ws2 = WorkingSet::from_parts(src.clone(), src).unwrap();
        assert_eq!(
            ws2.clone().apply_edit(
                &derived,
                &create("fixture_support/__init__.py", "TOL = 9\n")
            ),
            Err(EditDenial::OracleProtected)
        );
        assert_eq!(
            ws2.clone()
                .apply_edit(&derived, &create("src/fixture_support.py", "TOL = 9\n")),
            Ok(vec!["src/fixture_support.py".into()])
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn task_workspace_from_capture_and_unverified_repository_label() {
        use crate::knowledge_retrieval::TrustedSource;
        use std::fs;
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("project");
        fs::create_dir_all(root.join(".git/refs/heads/feature")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        let primary = b"def f():\n    return 1\n";
        fs::write(root.join("src/calc.py"), primary).unwrap();
        fs::write(root.join("README.md"), b"# r\n").unwrap();
        let scratch = t.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        let snap = SnapshotPolicy::new(vec![root.clone()], scratch).unwrap();
        let source = TrustedSource {
            source_id: "repo:test".into(),
            source_version: "inbharat.pai.stage5.selection.v1".into(),
            source_commit: "a".repeat(64),
            file_digest: hash(primary),
        };
        let selection = vec!["src/calc.py".to_owned(), "README.md".to_owned()];
        let snapshot = snap
            .capture(&root, source, "src/calc.py", &selection)
            .unwrap();
        let captured = CapturedSelection {
            files: files(&[("src/calc.py", primary), ("README.md", b"# r\n")]),
            snapshot,
        };
        let ws = WorkingSet::from_capture(&captured);
        let m = ws.manifest();
        assert_eq!(&m.base, &captured.snapshot.binding().files);
        assert_eq!(
            m.base_sha256,
            manifest_sha256(&captured.snapshot.binding().files)
        );
        assert_eq!(m.base_sha256, m.current_sha256);
        // Repository label: .git files only, labelled unverified.
        let label =
            |p: &SnapshotPolicy| repository_label(p, &root, "~/project".into(), "repo:test".into());
        let commit = "0123456789abcdef0123456789abcdef01234567";
        fs::write(root.join(".git/HEAD"), b"ref: refs/heads/feature/x\n").unwrap();
        fs::write(
            root.join(".git/refs/heads/feature/x"),
            format!("{commit}\n"),
        )
        .unwrap();
        let l = label(&snap);
        assert_eq!(
            (
                l.branch.as_deref(),
                l.head_commit.as_deref(),
                l.label_source
            ),
            (
                Some("feature/x"),
                Some(commit),
                LabelSource::GitFilesUnverified
            )
        );
        fs::remove_file(root.join(".git/refs/heads/feature/x")).unwrap();
        fs::write(
            root.join(".git/packed-refs"),
            format!("# pack-refs with: peeled\n{commit} refs/heads/feature/x\n"),
        )
        .unwrap();
        assert_eq!(label(&snap).head_commit.as_deref(), Some(commit));
        fs::write(root.join(".git/HEAD"), format!("{commit}\n")).unwrap();
        let l = label(&snap);
        assert_eq!(
            (l.branch, l.head_commit.as_deref(), l.label_source),
            (None, Some(commit), LabelSource::GitFilesUnverified)
        );
        for bad in [
            "garbage\n".to_owned(),
            "ref: refs/heads/../../etc\n".into(),
            "ref: refs/heads/-x\n".into(),
            format!("ref: refs/heads/{}\n", "a".repeat(5000)),
        ] {
            fs::write(root.join(".git/HEAD"), bad).unwrap();
            assert_eq!(label(&snap).label_source, LabelSource::Unknown);
        }
        fs::remove_file(root.join(".git/HEAD")).unwrap();
        assert_eq!(label(&snap).label_source, LabelSource::Unknown);
        let outside = t.path().join("outside-HEAD");
        fs::write(&outside, b"ref: refs/heads/main\n").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".git/HEAD")).unwrap();
        assert_eq!(
            label(&snap).label_source,
            LabelSource::Unknown,
            "symlinked HEAD is never followed"
        );
        // Root not approved → Unknown, nothing read.
        let other = t.path().join("other-root");
        fs::create_dir(&other).unwrap();
        let s2 = t.path().join("scratch2");
        fs::create_dir(&s2).unwrap();
        let unapproved = SnapshotPolicy::new(vec![other], s2).unwrap();
        assert_eq!(label(&unapproved).label_source, LabelSource::Unknown);
    }
}
