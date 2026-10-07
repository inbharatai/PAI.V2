//! Stage 5 task worktree: the ONLY host write of a coding task (design §4.2,
//! owner B). Generated code never writes here; the orchestrator (owner A)
//! writes accepted content after a main-window `UiApplyEvent`.
//!
//! Location: always a NEW directory `pai-task-<id8>` (mkdirat 0700, EEXIST is an
//! error) directly under a canonical, symlink-free `base_dir`, never inside or
//! equal to a `deny_within` entry (source roots, install dir, vault, scratch),
//! never in system trees, never directly in the home directory.
//!
//! Every operation is fd-relative beneath the pinned root fd. Final files are
//! read through `O_NOFOLLOW|O_NONBLOCK` fds that must be regular, single-link,
//! same-device and fstat-stable. Writes go to `.pai-tmp-<nonce>`
//! (`O_CREAT|O_EXCL|O_NOFOLLOW`, 0600, fsync); creation renames with
//! `RENAME_NOREPLACE`; replacement swaps with `RENAME_EXCHANGE` and then verifies
//! that the DISPLACED inode is exactly the expected pre-image (otherwise it swaps
//! back and fails); removal renames the target aside and verifies it before
//! unlinking. The directory is fsynced and the post-image re-hashed.
//!
//! Directory identity (review R2 F1): the worktree is always NEW, so it only
//! ever uses directories it created itself. The root is pinned by fd and by the
//! binding's (dev, ino). Every directory the worktree creates is made under an
//! unpredictable `.pai-dir-<nonce>` name (mkdirat 0700), opened `O_NOFOLLOW`,
//! checked (a directory on the root's device, owned by the root's owner, no
//! group/other bits, empty, birth time not before the root's where the
//! filesystem reports one), RECORDED by (dev, ino) and only then published with
//! `RENAME_NOREPLACE`. Each path step opens one component
//! `O_DIRECTORY|O_NOFOLLOW` relative to the previous fd and is refused with
//! `IdentityChanged` unless the opened directory's (dev, ino) equals the
//! identity recorded for that path (a symlink is `Symlink`, a non-directory
//! `NotRegular`). So a directory renamed, exchanged or re-created into the tree
//! (a real same-filesystem directory from outside, or one made out of band
//! inside it) is never read from or written into, within the residuals below.
//! After every write or removal the whole chain is re-opened from the root and
//! compared again before `Ok`: if a concurrent rename moved one of the
//! worktree's directories, the call returns `IdentityChanged` (state unknown for
//! owner A; the bytes are in the worktree's own directory inode, wherever that
//! rename put it). The root itself is the worktree: renaming the whole root
//! moves the worktree, it never redirects a write.
//!
//! Where the identities live: the binding persisted by owner A is unchanged (it
//! is written once, at creation). The identities are kept for the process
//! lifetime in a registry keyed by the binding, so every `reopen` in the same
//! process uses exactly the recorded identities (of directories made in this
//! process, or adopted at its first reopen of that worktree; never anything
//! else). On the first `reopen` after a restart they are re-derived by an
//! fd-relative `O_NOFOLLOW` walk from the pinned root that adopts only
//! directories carrying kernel evidence of having been made after the root by
//! its owner the way this module makes them: same device and owner, no
//! group/other bits, and a statx birth time not before the root's (user space
//! cannot set a birth time). On a filesystem without birth times nothing is
//! adopted, so after a restart every path through an existing directory fails
//! closed (`IdentityChanged`).
//!
//! Residuals (same-UID host processes, the design's trusted-boundary residual):
//! a directory present at that post-restart re-derivation that was itself made
//! after the root by the same user with mode 0700 (exchanged in, or made out of
//! band) is adopted; a process that learns a `.pai-dir-<nonce>` name (e.g. via
//! inotify) can substitute an empty directory of its own before it is opened;
//! inode-number reuse after a recorded directory is deleted is not detected; a
//! recorded directory moved out and back between the write and the re-check is
//! accepted (the bytes are in it); anything may be moved after `Ok`.
//! Filesystems without renameat2 fall back to check-then-rename, for files and
//! for publishing a new directory alike. An interrupted directory creation can
//! leave an empty `.pai-dir-*`. Non-Linux: `Unavailable`.
//!
//! Cleanup ownership: a helper only ever unlinks a name while it still refers
//! to the exact inode (dev, ino) it owns — its own temp (pinned by fstat at
//! O_EXCL creation), the displaced/moved-aside inode it just verified to be
//! the expected pre-image, or its own still-empty `.pai-dir-*` directory.
//! Anything else found under that name (a concurrent writer's file, a symlink,
//! a directory) is left in place and the operation reports `Io` (state unknown,
//! owner A reconciles); it is never deleted. The remaining stat-then-unlink
//! window is the same same-UID residual.

use crate::isolation::hash;
use crate::task_workspace::{validate_path, HARD_MAX_FILE_BYTES};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

pub(crate) use crate::task_ledger::TaskId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePolicy {
    /// Canonical, existing, symlink-free directory that receives `pai-task-<id8>`.
    pub base_dir: PathBuf,
    /// The new worktree may not be inside/equal to (or contain) any of these:
    /// install dir, vault root, every SnapshotPolicy root, scratch. (The home
    /// directory ITSELF and system trees are denied built-in, see module docs.)
    pub deny_within: Vec<PathBuf>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeBinding {
    pub path: String,
    pub dev: u64,
    pub ino: u64,
    pub created_at_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeError {
    /// Non-Linux: no fd-safe implementation; apply fails closed.
    Unavailable,
    DeniedLocation,
    /// The root, or a directory on the path, is not the recorded one (module
    /// docs). From `replace_atomic` / `remove` after a write: state unknown.
    IdentityChanged,
    PreImageMismatch {
        path: String,
    },
    Symlink,
    NotRegular,
    Io,
}
impl fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "worktree: {self:?}")
    }
}
impl std::error::Error for WorktreeError {}

/// Implemented by [`LinuxWorktree`]; in-memory fakes in tests.
pub trait WorktreeIo {
    fn binding(&self) -> &WorktreeBinding;
    /// fd-relative, O_NOFOLLOW. `None` = absent.
    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>, WorktreeError>;
    /// `expected_pre`: lowercase SHA-256 of the current file, or `None` = must be absent.
    fn replace_atomic(
        &self,
        rel: &str,
        bytes: &[u8],
        expected_pre: Option<&str>,
    ) -> Result<(), WorktreeError>;
    fn remove(&self, rel: &str, expected_pre: &str) -> Result<(), WorktreeError>;
}

#[derive(Debug)]
pub struct LinuxWorktree {
    binding: WorktreeBinding,
    #[cfg(target_os = "linux")]
    root: std::os::fd::OwnedFd,
    /// Kernel facts about the root used as creation evidence.
    #[cfg(target_os = "linux")]
    origin: fdops::Origin,
    /// Recorded identities of the directories below the root, shared through
    /// the process registry by every instance of this worktree.
    #[cfg(target_os = "linux")]
    dirs: registry::Shared,
}

fn worktree_name(task: &TaskId) -> Result<String, WorktreeError> {
    let id = task.as_str();
    let id8 = id.get(..8).ok_or(WorktreeError::DeniedLocation)?;
    if !id8
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(WorktreeError::DeniedLocation);
    }
    Ok(format!("pai-task-{id8}"))
}
fn split(rel: &str) -> Result<(Vec<&str>, &str), WorktreeError> {
    validate_path(rel).map_err(|_| WorktreeError::DeniedLocation)?;
    let mut parts: Vec<&str> = rel.split('/').collect();
    let name = parts.pop().ok_or(WorktreeError::DeniedLocation)?;
    Ok((parts, name))
}

impl LinuxWorktree {
    /// Creates `<base_dir>/pai-task-<id8>` (new, 0700) after the location policy.
    pub fn create(policy: &WorktreePolicy, task: &TaskId) -> Result<Self, WorktreeError> {
        #[cfg(target_os = "linux")]
        {
            let name = worktree_name(task)?;
            let target = location::check(policy, &name)?;
            let path = target
                .to_str()
                .ok_or(WorktreeError::DeniedLocation)?
                .to_owned();
            let (root, st) = fdops::create_dir(&policy.base_dir, &name)?;
            let created_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            let binding = WorktreeBinding {
                path,
                dev: fdops::wide(st.st_dev),
                ino: fdops::wide(st.st_ino),
                created_at_ms,
            };
            // A new, empty directory: nothing below it is recorded yet.
            let dirs = registry::fresh(&binding);
            Ok(Self {
                binding,
                origin: fdops::origin(&root, &st),
                root,
                dirs,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (policy, worktree_name(task));
            Err(WorktreeError::Unavailable)
        }
    }
    /// Re-opens a recorded worktree; its (dev, ino) must match the binding.
    pub fn reopen(policy: &WorktreePolicy, b: &WorktreeBinding) -> Result<Self, WorktreeError> {
        #[cfg(target_os = "linux")]
        {
            let path = std::path::Path::new(&b.path);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or(WorktreeError::DeniedLocation)?;
            let expected = name
                .strip_prefix("pai-task-")
                .filter(|id| {
                    id.len() == 8
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
                .is_some();
            if !expected || path.parent() != Some(policy.base_dir.as_path()) {
                return Err(WorktreeError::DeniedLocation);
            }
            if location::check(policy, name)? != path {
                return Err(WorktreeError::DeniedLocation);
            }
            let (root, st) =
                fdops::open_abs_dir(path).map_err(|_| WorktreeError::IdentityChanged)?;
            if (fdops::wide(st.st_dev), fdops::wide(st.st_ino)) != (b.dev, b.ino) {
                return Err(WorktreeError::IdentityChanged);
            }
            let origin = fdops::origin(&root, &st);
            // Same process: exactly the recorded identities. First reopen after
            // a restart: re-derived from kernel evidence (module docs).
            let dirs = registry::reopen(b, || fdops::derive(&root, &origin));
            Ok(Self {
                binding: b.clone(),
                root,
                origin,
                dirs,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (policy, b);
            Err(WorktreeError::Unavailable)
        }
    }
}

#[cfg(target_os = "linux")]
impl LinuxWorktree {
    /// [`fdops::parent`] against this worktree's recorded directory identities.
    fn walk(
        &self,
        dirs: &[&str],
        create: bool,
    ) -> Result<Option<(std::os::fd::OwnedFd, Vec<fdops::Step>)>, WorktreeError> {
        let mut recorded = self
            .dirs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fdops::parent(&self.root, &self.origin, dirs, create, &mut recorded)
    }
}

impl WorktreeIo for LinuxWorktree {
    fn binding(&self) -> &WorktreeBinding {
        &self.binding
    }
    #[cfg(target_os = "linux")]
    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>, WorktreeError> {
        let (dirs, name) = split(rel)?;
        let Some((dir, _)) = self.walk(&dirs, false)? else {
            return Ok(None);
        };
        match fdops::read_entry(&dir, name, self.binding.dev)? {
            fdops::Entry::Missing => Ok(None),
            fdops::Entry::File(bytes) => Ok(Some(bytes)),
        }
    }
    #[cfg(target_os = "linux")]
    fn replace_atomic(
        &self,
        rel: &str,
        bytes: &[u8],
        expected_pre: Option<&str>,
    ) -> Result<(), WorktreeError> {
        let (dirs, name) = split(rel)?;
        if bytes.len() > HARD_MAX_FILE_BYTES {
            return Err(WorktreeError::Io);
        }
        let dev = self.binding.dev;
        let (dir, chain) = self.walk(&dirs, true)?.ok_or(WorktreeError::Io)?;
        fdops::check_pre(&dir, name, dev, expected_pre, rel)?;
        let tmp = fdops::write_temp(&dir, bytes)?;
        // Cleanup ownership: each helper removes the temp name only while it is
        // still OUR inode (or the verified, displaced pre-image inode), never
        // anything else; a foreign entry there is left in place (`Io`).
        match expected_pre {
            None => fdops::create_noreplace(&dir, &tmp, name, rel)?,
            Some(pre) => fdops::exchange_verified(&dir, &tmp, name, dev, pre, rel)?,
        }
        fdops::sync(&dir)?;
        match fdops::read_entry(&dir, name, dev)? {
            fdops::Entry::File(now) if hash(&now) == hash(bytes) => {}
            _ => return Err(WorktreeError::Io),
        }
        // The bytes are in `dir` (a recorded directory); it must still be linked
        // at its path below the root before this reports success.
        fdops::verify(&self.root, &chain)
    }
    #[cfg(target_os = "linux")]
    fn remove(&self, rel: &str, expected_pre: &str) -> Result<(), WorktreeError> {
        let (dirs, name) = split(rel)?;
        let dev = self.binding.dev;
        let mismatch = || WorktreeError::PreImageMismatch {
            path: rel.to_owned(),
        };
        let (dir, chain) = self.walk(&dirs, false)?.ok_or_else(mismatch)?;
        fdops::check_pre(&dir, name, dev, Some(expected_pre), rel)?;
        fdops::remove_verified(&dir, name, dev, expected_pre, rel)?;
        fdops::sync(&dir)?;
        fdops::verify(&self.root, &chain)
    }
    #[cfg(not(target_os = "linux"))]
    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>, WorktreeError> {
        let _ = split(rel)?;
        Err(WorktreeError::Unavailable)
    }
    #[cfg(not(target_os = "linux"))]
    fn replace_atomic(
        &self,
        rel: &str,
        bytes: &[u8],
        expected_pre: Option<&str>,
    ) -> Result<(), WorktreeError> {
        let _ = (split(rel)?, bytes.len() > HARD_MAX_FILE_BYTES, expected_pre);
        Err(WorktreeError::Unavailable)
    }
    #[cfg(not(target_os = "linux"))]
    fn remove(&self, rel: &str, expected_pre: &str) -> Result<(), WorktreeError> {
        let _ = (split(rel)?, expected_pre);
        Err(WorktreeError::Unavailable)
    }
}

/// One file of an apply / revert: `post = None` removes it.
#[derive(Clone, PartialEq, Eq)]
pub struct WorktreeChange {
    pub path: String,
    pub expected_pre: Option<String>,
    pub post: Option<Vec<u8>>,
}
impl fmt::Debug for WorktreeChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorktreeChange")
            .field("path", &self.path)
            .field("expected_pre", &self.expected_pre)
            .field("post_sha256", &self.post.as_ref().map(|b| hash(b)))
            .finish()
    }
}

/// Changes taking a worktree from observed `pre` hashes to `post` content
/// (paths only in `pre` are removed; unchanged paths are omitted).
pub fn plan_changes(
    pre: &BTreeMap<String, Option<String>>,
    post: &BTreeMap<String, Option<Vec<u8>>>,
) -> Vec<WorktreeChange> {
    let paths: BTreeSet<&String> = pre.keys().chain(post.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let expected_pre = pre.get(path).cloned().flatten();
            let post = post.get(path).cloned().flatten();
            (post.as_ref().map(|b| hash(b)) != expected_pre).then(|| WorktreeChange {
                path: path.clone(),
                expected_pre,
                post,
            })
        })
        .collect()
}
/// Revert an applied post-image back to the captured base: files created by the
/// task are removed, changed/deleted base files restored.
pub fn revert_plan(
    applied_post: &BTreeMap<String, Option<String>>,
    base: &BTreeMap<String, Vec<u8>>,
) -> Vec<WorktreeChange> {
    let mut post: BTreeMap<String, Option<Vec<u8>>> =
        applied_post.keys().map(|p| (p.clone(), None)).collect();
    for (path, bytes) in base {
        post.insert(path.clone(), Some(bytes.clone()));
    }
    plan_changes(applied_post, &post)
}
/// fd-safe hashes of `paths` (reads only; for restart reconciliation).
pub fn observe(
    io: &dyn WorktreeIo,
    paths: &[String],
) -> Result<BTreeMap<String, Option<String>>, WorktreeError> {
    paths
        .iter()
        .map(|p| Ok((p.clone(), io.read(p)?.map(|b| hash(&b)))))
        .collect()
}
/// Applies `changes` with a pre-image check of EVERY path before ANY write, then
/// per-file atomic writes, then a post-image check of every path. A failure
/// part-way returns the error; the ledger's StepIntent (pre/post images) lets
/// owner A reconcile (design §3.5).
pub fn apply_changes(
    io: &dyn WorktreeIo,
    changes: &[WorktreeChange],
) -> Result<Vec<String>, WorktreeError> {
    let mut seen = BTreeSet::new();
    for c in changes {
        validate_path(&c.path).map_err(|_| WorktreeError::DeniedLocation)?;
        if !seen.insert(c.path.as_str()) {
            return Err(WorktreeError::DeniedLocation);
        }
        if c.post
            .as_ref()
            .is_some_and(|b| b.len() > HARD_MAX_FILE_BYTES)
        {
            return Err(WorktreeError::Io);
        }
    }
    for c in changes {
        if io.read(&c.path)?.map(|b| hash(&b)) != c.expected_pre {
            return Err(WorktreeError::PreImageMismatch {
                path: c.path.clone(),
            });
        }
    }
    let mut applied = Vec::new();
    for c in changes {
        match (&c.post, &c.expected_pre) {
            (Some(bytes), pre) if Some(hash(bytes)) != *pre => {
                io.replace_atomic(&c.path, bytes, pre.as_deref())?;
            }
            (None, Some(pre)) => io.remove(&c.path, pre)?,
            _ => continue,
        }
        applied.push(c.path.clone());
    }
    for c in changes {
        if io.read(&c.path)?.map(|b| hash(&b)) != c.post.as_ref().map(|b| hash(b)) {
            return Err(WorktreeError::Io);
        }
    }
    Ok(applied)
}

#[cfg(target_os = "linux")]
mod location {
    use super::WorktreeError;
    use std::path::{Component, Path, PathBuf};

    /// Never a worktree location (in addition to the policy's deny list).
    const SYSTEM: &[&str] = &[
        "/usr", "/etc", "/lib", "/lib32", "/lib64", "/libx32", "/bin", "/sbin", "/boot", "/proc",
        "/sys", "/dev", "/run", "/var/run", "/snap",
    ];
    fn lexical(p: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    }
    /// The location policy; returns `<base_dir>/<name>`.
    pub(super) fn check(
        policy: &super::WorktreePolicy,
        name: &str,
    ) -> Result<PathBuf, WorktreeError> {
        let deny = WorktreeError::DeniedLocation;
        let base = &policy.base_dir;
        // Canonical == symlink-free, no `.`/`..`, existing; never "/".
        if !base.is_absolute()
            || std::fs::canonicalize(base).ok().as_ref() != Some(base)
            || base.components().count() < 2
        {
            return Err(deny);
        }
        let home = std::env::var_os("HOME").and_then(|h| std::fs::canonicalize(h).ok());
        if home.as_ref() == Some(base) || base == Path::new("/home") || base == Path::new("/root") {
            return Err(deny);
        }
        let target = base.join(name);
        if SYSTEM.iter().any(|s| target.starts_with(s)) {
            return Err(deny);
        }
        for entry in &policy.deny_within {
            if !entry.is_absolute() {
                return Err(deny);
            }
            let entry = std::fs::canonicalize(entry).unwrap_or_else(|_| lexical(entry));
            if target.starts_with(&entry) || entry.starts_with(&target) {
                return Err(deny);
            }
        }
        Ok(target)
    }
}

/// Process-lifetime record of every worktree's directory identities (module
/// docs), keyed by its binding. The binding owner A persists is unchanged.
#[cfg(target_os = "linux")]
mod registry {
    use super::{fdops::DirMap, WorktreeBinding};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, PoisonError};

    pub(super) type Shared = Arc<Mutex<DirMap>>;
    type Key = (u64, u64, u64);
    static RECORDS: Mutex<BTreeMap<Key, Shared>> = Mutex::new(BTreeMap::new());

    fn key(b: &WorktreeBinding) -> Key {
        (b.dev, b.ino, b.created_at_ms)
    }
    /// A newly created worktree: an empty record (replacing any stale record
    /// of a reused root inode).
    pub(super) fn fresh(b: &WorktreeBinding) -> Shared {
        let shared = Shared::default();
        RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key(b), shared.clone());
        shared
    }
    /// The record this process keeps for `b`; if there is none (first reopen
    /// after a restart) the one `derive` re-derives.
    pub(super) fn reopen(b: &WorktreeBinding, derive: impl FnOnce() -> DirMap) -> Shared {
        if let Some(shared) = RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key(b))
        {
            return shared.clone();
        }
        let derived = Arc::new(Mutex::new(derive()));
        RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key(b))
            .or_insert(derived)
            .clone()
    }
    /// Test-only: drop the record, as a process restart would.
    #[cfg(test)]
    pub(super) fn forget(b: &WorktreeBinding) {
        RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key(b));
    }
}

#[cfg(target_os = "linux")]
mod fdops {
    use super::WorktreeError;
    use crate::isolation::{hash, nonce};
    use crate::task_workspace::{validate_path, MAX_PATH_DEPTH};
    use rustix::fs::{
        fstat, fsync, mkdirat, openat, renameat, renameat_with, statat, statx, unlinkat, AtFlags,
        Dir, FileType, Mode, OFlags, RenameFlags, Stat, StatxFlags, CWD,
    };
    use rustix::io::Errno;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::AsFd;
    use std::os::fd::OwnedFd;
    use std::path::{Component, Path};

    // Test-only, per-thread switches (thread-local so parallel tests never
    // observe each other): force the no-renameat2 fallback, and a hook called
    // at fixed points with the temp/aside name to inject deterministic
    // same-UID interference ("commit", "restore", "cleanup").
    #[cfg(test)]
    pub(super) type Hook = Box<dyn FnMut(&'static str, &str)>;
    #[cfg(test)]
    thread_local! {
        pub(super) static FORCE_PLAIN_RENAME: std::cell::Cell<bool> =
            const { std::cell::Cell::new(false) };
        pub(super) static HOOK: std::cell::RefCell<Option<Hook>> =
            const { std::cell::RefCell::new(None) };
    }
    #[inline]
    fn hook(_point: &'static str, _name: &str) {
        #[cfg(test)]
        {
            let taken = HOOK.with(|h| h.borrow_mut().take());
            if let Some(mut f) = taken {
                f(_point, _name);
                HOOK.with(|h| *h.borrow_mut() = Some(f));
            }
        }
    }

    /// (dev, ino) of an inode this module created or verified.
    pub(super) type Ident = (u64, u64);
    fn ident(st: &Stat) -> Ident {
        (wide(st.st_dev), wide(st.st_ino))
    }
    /// Our `.pai-tmp-<nonce>` file and the identity pinned at creation.
    pub(super) struct Temp {
        name: String,
        owned: Ident,
    }
    /// Recorded identities of the directories below a worktree root: relative
    /// directory path (`src`, `src/pkg`) -> (dev, ino). The root is the binding.
    pub(super) type DirMap = BTreeMap<String, Ident>;
    /// One traversal step: the component name and the recorded identity it had.
    pub(super) type Step = (String, Ident);
    /// Kernel facts about the worktree root used as creation evidence.
    #[derive(Debug, Clone, Copy)]
    pub(super) struct Origin {
        dev: u64,
        uid: u64,
        /// statx birth time (sec, nsec); `None` = not reported by the filesystem.
        born: Option<(i64, u32)>,
    }
    /// Re-derivation bounds: directory entries examined and directories adopted.
    const DERIVE_MAX_ENTRIES: usize = 65_536;
    const DERIVE_MAX_DIRS: usize = 4_096;

    pub(super) fn wide<T: Into<u64>>(v: T) -> u64 {
        v.into()
    }
    fn dir_flags() -> OFlags {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }
    fn map(e: Errno) -> WorktreeError {
        if e == Errno::LOOP {
            WorktreeError::Symlink
        } else if e == Errno::NOTDIR {
            WorktreeError::NotRegular
        } else {
            WorktreeError::Io
        }
    }
    fn kind(st: &Stat) -> FileType {
        FileType::from_raw_mode(st.st_mode)
    }
    /// `openat(O_DIRECTORY|O_NOFOLLOW)` reports a symlink as ENOTDIR or ELOOP;
    /// name it precisely (diagnostic only — both are denials).
    fn open_subdir<Fd: AsFd>(dir: Fd, name: &std::ffi::OsStr) -> Result<OwnedFd, Errno> {
        openat(&dir, name, dir_flags(), Mode::empty())
    }
    fn classify<Fd: AsFd>(dir: Fd, name: &std::ffi::OsStr, e: Errno) -> WorktreeError {
        if e == Errno::NOTDIR || e == Errno::LOOP {
            match statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) if kind(&st) == FileType::Symlink => WorktreeError::Symlink,
                _ => WorktreeError::NotRegular,
            }
        } else {
            map(e)
        }
    }
    /// Absolute directory opened from "/" one component at a time, O_NOFOLLOW.
    pub(super) fn open_abs_dir(path: &Path) -> Result<(OwnedFd, Stat), WorktreeError> {
        let mut parts = path.components();
        if parts.next() != Some(Component::RootDir) {
            return Err(WorktreeError::DeniedLocation);
        }
        let mut fd = openat(CWD, "/", dir_flags(), Mode::empty()).map_err(map)?;
        for part in parts {
            let Component::Normal(name) = part else {
                return Err(WorktreeError::DeniedLocation);
            };
            fd = open_subdir(&fd, name).map_err(|e| classify(&fd, name, e))?;
        }
        let st = fstat(&fd).map_err(map)?;
        if kind(&st) != FileType::Directory {
            return Err(WorktreeError::NotRegular);
        }
        Ok((fd, st))
    }
    /// mkdirat a NEW 0700 directory beneath the symlink-free base, re-open it
    /// O_NOFOLLOW and pin it. EEXIST is a location error (never reuse).
    pub(super) fn create_dir(base: &Path, name: &str) -> Result<(OwnedFd, Stat), WorktreeError> {
        let (base_fd, base_st) = open_abs_dir(base).map_err(|_| WorktreeError::DeniedLocation)?;
        match mkdirat(&base_fd, name, Mode::from_raw_mode(0o700)) {
            Ok(()) => {}
            Err(Errno::EXIST) => return Err(WorktreeError::DeniedLocation),
            Err(e) => return Err(map(e)),
        }
        let pinned = (|| {
            let root = openat(&base_fd, name, dir_flags(), Mode::empty()).map_err(map)?;
            let st = fstat(&root).map_err(map)?;
            if kind(&st) != FileType::Directory
                || st.st_dev != base_st.st_dev
                || st.st_mode & 0o077 != 0
            {
                return Err(WorktreeError::IdentityChanged);
            }
            fsync(&base_fd).map_err(map)?;
            Ok((root, st))
        })();
        if pinned.is_err() {
            // Best effort: only ever removes an EMPTY directory.
            let _ = unlinkat(&base_fd, name, AtFlags::REMOVEDIR);
        }
        pinned
    }
    pub(super) fn origin(root: &OwnedFd, st: &Stat) -> Origin {
        Origin {
            dev: wide(st.st_dev),
            uid: wide(st.st_uid),
            born: born(root),
        }
    }
    /// statx birth time of `fd` (kernel-set at creation), if reported.
    fn born<Fd: AsFd>(fd: Fd) -> Option<(i64, u32)> {
        let sx = statx(fd, "", AtFlags::EMPTY_PATH, StatxFlags::BTIME).ok()?;
        (sx.stx_mask & StatxFlags::BTIME.bits() != 0)
            .then_some((sx.stx_btime.tv_sec, sx.stx_btime.tv_nsec))
    }
    /// `fd` is an EMPTY directory (only `.` and `..`).
    fn is_empty(fd: &OwnedFd) -> bool {
        Dir::read_from(fd).is_ok_and(|mut entries| {
            entries.all(|e| e.is_ok_and(|e| matches!(e.file_name().to_bytes(), b"." | b"..")))
        })
    }
    /// A directory on the root's device, owned by the root's owner, with no
    /// group/other permission bits (as this module's mkdirat 0700 makes them).
    fn made_like_ours(st: &Stat, origin: &Origin) -> bool {
        kind(st) == FileType::Directory
            && wide(st.st_dev) == origin.dev
            && wide(st.st_uid) == origin.uid
            && st.st_mode & 0o077 == 0
    }
    /// Parent directory of a relative path beneath `root`, opened one
    /// component at a time `O_DIRECTORY|O_NOFOLLOW` relative to the previous
    /// fd. Every opened directory must be on the root's device (else
    /// `NotRegular`) and its (dev, ino) must equal the identity RECORDED for
    /// that path (else `IdentityChanged`): a directory renamed, exchanged or
    /// made out of band into the tree is never used. With `create`, a missing
    /// directory is made by [`make_dir`] and recorded. `Ok(None)`: a component
    /// is missing and `create` is false. Also returns the chain for [`verify`].
    pub(super) fn parent(
        root: &OwnedFd,
        origin: &Origin,
        dirs: &[&str],
        create: bool,
        recorded: &mut DirMap,
    ) -> Result<Option<(OwnedFd, Vec<Step>)>, WorktreeError> {
        let mut current = openat(root, ".", dir_flags(), Mode::empty()).map_err(map)?;
        let mut rel = String::new();
        let mut chain = Vec::with_capacity(dirs.len());
        for name in dirs {
            if !rel.is_empty() {
                rel.push('/');
            }
            rel.push_str(name);
            let os = OsStr::new(*name);
            let next = match open_subdir(&current, os) {
                Ok(fd) => fd,
                Err(Errno::NOENT) if create => match make_dir(&current, name, origin)? {
                    Some((fd, made)) => {
                        recorded.insert(rel.clone(), made);
                        fd
                    }
                    // The name was taken meanwhile: checked like any entry.
                    None => open_subdir(&current, os).map_err(|e| classify(&current, os, e))?,
                },
                Err(Errno::NOENT) => return Ok(None),
                Err(e) => return Err(classify(&current, os, e)),
            };
            let st = fstat(&next).map_err(map)?;
            if kind(&st) != FileType::Directory || wide(st.st_dev) != origin.dev {
                return Err(WorktreeError::NotRegular);
            }
            let id = ident(&st);
            if recorded.get(&rel) != Some(&id) {
                return Err(WorktreeError::IdentityChanged);
            }
            chain.push(((*name).to_owned(), id));
            current = next;
        }
        Ok(Some((current, chain)))
    }
    /// Makes directory `name` in `parent` so that its identity is known before
    /// the name exists: mkdirat 0700 under an unpredictable `.pai-dir-<nonce>`
    /// name, opened `O_NOFOLLOW` and checked ([`made_like_ours`], empty, birth
    /// time not before the root's when reported), then published with
    /// `RENAME_NOREPLACE`. `Ok(None)`: `name` appeared meanwhile (our empty
    /// directory is removed again while it is still ours). An entry that is not
    /// provably ours at the temp name is left in place (`IdentityChanged`).
    fn make_dir(
        parent: &OwnedFd,
        name: &str,
        origin: &Origin,
    ) -> Result<Option<(OwnedFd, Ident)>, WorktreeError> {
        let mut tmp = None;
        for _ in 0..4 {
            let candidate = format!(".pai-dir-{}", nonce().map_err(|_| WorktreeError::Io)?);
            match mkdirat(parent, candidate.as_str(), Mode::from_raw_mode(0o700)) {
                Ok(()) => {
                    tmp = Some(candidate);
                    break;
                }
                Err(Errno::EXIST) => {}
                Err(e) => return Err(map(e)),
            }
        }
        let tmp = tmp.ok_or(WorktreeError::Io)?;
        hook("mkdir", &tmp);
        let fd =
            open_subdir(parent, OsStr::new(&tmp)).map_err(|_| WorktreeError::IdentityChanged)?;
        let st = fstat(&fd).map_err(map)?;
        let born_ok = match (origin.born, born(&fd)) {
            (Some(root_born), Some(b)) => b >= root_born,
            (Some(_), None) => false,
            (None, _) => true,
        };
        if !made_like_ours(&st, origin) || !born_ok || !is_empty(&fd) {
            return Err(WorktreeError::IdentityChanged);
        }
        let made = ident(&st);
        match rename2(parent, &tmp, name, RenameFlags::NOREPLACE) {
            Ok(()) => Ok(Some((fd, made))),
            Err(Errno::EXIST) => {
                rmdir_owned(parent, &tmp, made);
                Ok(None)
            }
            // Residual fallback (no renameat2 on this filesystem): check-then-rename.
            Err(e) if unsupported(e) => match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                Err(Errno::NOENT) => match renameat(parent, tmp.as_str(), parent, name) {
                    Ok(()) => Ok(Some((fd, made))),
                    Err(e) => {
                        rmdir_owned(parent, &tmp, made);
                        Err(map(e))
                    }
                },
                _ => {
                    rmdir_owned(parent, &tmp, made);
                    Ok(None)
                }
            },
            Err(e) => {
                rmdir_owned(parent, &tmp, made);
                Err(map(e))
            }
        }
    }
    /// Cleanup ownership for our temp directory: removed only while the name
    /// is still exactly our directory (REMOVEDIR also requires it be empty).
    fn rmdir_owned(parent: &OwnedFd, name: &str, owned: Ident) {
        if let Ok(st) = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            if kind(&st) == FileType::Directory && ident(&st) == owned {
                let _ = unlinkat(parent, name, AtFlags::REMOVEDIR);
            }
        }
    }
    /// After a write: re-opens the chain from the root by name (`O_NOFOLLOW`)
    /// and requires every step to still be the recorded identity, i.e. the
    /// directory written into is still linked at its path below the root.
    /// Anything else is `IdentityChanged` (state unknown).
    pub(super) fn verify(root: &OwnedFd, chain: &[Step]) -> Result<(), WorktreeError> {
        hook("verify", chain.last().map_or("", |(name, _)| name.as_str()));
        let mut current = openat(root, ".", dir_flags(), Mode::empty()).map_err(map)?;
        for (name, id) in chain {
            let next = open_subdir(&current, OsStr::new(name))
                .map_err(|_| WorktreeError::IdentityChanged)?;
            let st = fstat(&next).map_err(|_| WorktreeError::IdentityChanged)?;
            if kind(&st) != FileType::Directory || ident(&st) != *id {
                return Err(WorktreeError::IdentityChanged);
            }
            current = next;
        }
        Ok(())
    }
    /// Re-derives the record of a worktree this process has not seen (first
    /// reopen after a restart): an fd-relative `O_NOFOLLOW` walk from the
    /// pinned root over names a valid worktree path can contain, adopting only
    /// directories (below adopted ones) that are [`made_like_ours`] AND have a
    /// birth time not before the root's. Bounded; nothing else is adopted, and
    /// what is not adopted is never traversed. No birth times: nothing.
    pub(super) fn derive(root: &OwnedFd, origin: &Origin) -> DirMap {
        let mut recorded = DirMap::new();
        if origin.born.is_some() {
            let mut budget = DERIVE_MAX_ENTRIES;
            adopt_below(root, "", origin, &mut recorded, &mut budget);
        }
        recorded
    }
    fn adopt_below(
        dir: &OwnedFd,
        rel: &str,
        origin: &Origin,
        recorded: &mut DirMap,
        budget: &mut usize,
    ) {
        let Ok(entries) = Dir::read_from(dir) else {
            return;
        };
        for entry in entries {
            if *budget == 0 || recorded.len() >= DERIVE_MAX_DIRS {
                return;
            }
            *budget -= 1;
            let Ok(entry) = entry else {
                return;
            };
            if !matches!(entry.file_type(), FileType::Directory | FileType::Unknown) {
                continue;
            }
            let Ok(name) = entry.file_name().to_str() else {
                continue;
            };
            if name == "." || name == ".." {
                continue;
            }
            let child = if rel.is_empty() {
                name.to_owned()
            } else {
                format!("{rel}/{name}")
            };
            // Only a directory that a valid file path can pass through.
            if child.split('/').count() >= MAX_PATH_DEPTH || validate_path(&child).is_err() {
                continue;
            }
            let Ok(sub) = open_subdir(dir, OsStr::new(name)) else {
                continue;
            };
            let Ok(st) = fstat(&sub) else {
                continue;
            };
            let born_ok = matches!((origin.born, born(&sub)), (Some(r), Some(b)) if b >= r);
            if made_like_ours(&st, origin) && born_ok {
                recorded.insert(child.clone(), ident(&st));
                adopt_below(&sub, &child, origin, recorded, budget);
            }
        }
    }
    pub(super) enum Entry {
        Missing,
        File(Vec<u8>),
    }
    fn stable(a: &Stat, b: &Stat) -> bool {
        (a.st_dev, a.st_ino, a.st_mode, a.st_nlink, a.st_size)
            == (b.st_dev, b.st_ino, b.st_mode, b.st_nlink, b.st_size)
            && (a.st_mtime, a.st_mtime_nsec, a.st_ctime, a.st_ctime_nsec)
                == (b.st_mtime, b.st_mtime_nsec, b.st_ctime, b.st_ctime_nsec)
    }
    /// Read one entry of `dir` without following symlinks: regular, nlink==1,
    /// same device, bounded, fstat-stable across the read.
    pub(super) fn read_entry(dir: &OwnedFd, name: &str, dev: u64) -> Result<Entry, WorktreeError> {
        Ok(match read_owned(dir, name, dev)? {
            None => Entry::Missing,
            Some((bytes, _)) => Entry::File(bytes),
        })
    }
    /// [`read_entry`] that also returns the identity of the inode read.
    fn read_owned(
        dir: &OwnedFd,
        name: &str,
        dev: u64,
    ) -> Result<Option<(Vec<u8>, Ident)>, WorktreeError> {
        let fd = match openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(e) => return Err(map(e)),
        };
        let before = fstat(&fd).map_err(map)?;
        if kind(&before) == FileType::Symlink {
            return Err(WorktreeError::Symlink);
        }
        if kind(&before) != FileType::RegularFile
            || before.st_nlink != 1
            || wide(before.st_dev) != dev
        {
            return Err(WorktreeError::NotRegular);
        }
        let size = u64::try_from(before.st_size).map_err(|_| WorktreeError::Io)?;
        let limit = super::HARD_MAX_FILE_BYTES as u64;
        if size > limit {
            return Err(WorktreeError::Io);
        }
        let mut file = File::from(fd);
        let mut bytes = Vec::new();
        (&mut file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| WorktreeError::Io)?;
        let after = fstat(&file).map_err(map)?;
        if bytes.len() as u64 != size || !stable(&before, &after) {
            return Err(WorktreeError::Io);
        }
        Ok(Some((bytes, ident(&before))))
    }
    pub(super) fn check_pre(
        dir: &OwnedFd,
        name: &str,
        dev: u64,
        expected: Option<&str>,
        rel: &str,
    ) -> Result<(), WorktreeError> {
        match (read_entry(dir, name, dev)?, expected) {
            (Entry::Missing, None) => Ok(()),
            (Entry::File(b), Some(h)) if hash(&b) == h => Ok(()),
            _ => Err(WorktreeError::PreImageMismatch {
                path: rel.to_owned(),
            }),
        }
    }
    /// `.pai-tmp-<nonce>`: O_CREAT|O_EXCL|O_NOFOLLOW, 0600, identity pinned by
    /// fstat of the creating fd, written and fsynced.
    pub(super) fn write_temp(dir: &OwnedFd, bytes: &[u8]) -> Result<Temp, WorktreeError> {
        for _ in 0..4 {
            let name = format!(".pai-tmp-{}", nonce().map_err(|_| WorktreeError::Io)?);
            let fd = match openat(
                dir,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            ) {
                Ok(fd) => fd,
                Err(Errno::EXIST) => continue,
                Err(e) => return Err(map(e)),
            };
            // Without a pinned identity the temp is never unlinked (left, `Io`).
            let owned = fstat(&fd).map_err(map)?;
            let tmp = Temp {
                name,
                owned: ident(&owned),
            };
            let mut file = File::from(fd);
            if file
                .write_all(bytes)
                .and_then(|()| file.sync_all())
                .is_err()
            {
                return Err(discard(dir, &tmp, WorktreeError::Io));
            }
            return Ok(tmp);
        }
        Err(WorktreeError::Io)
    }
    /// Cleanup ownership: unlink `name` only while it is still exactly the
    /// regular inode `owned`. Absent = nothing left = `Ok`. Anything else under
    /// the name is left in place and reported as `Io` (never deleted).
    pub(super) fn unlink_owned(
        dir: &OwnedFd,
        name: &str,
        owned: Ident,
    ) -> Result<(), WorktreeError> {
        hook("cleanup", name);
        match statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) if kind(&st) == FileType::RegularFile && ident(&st) == owned => {
                unlinkat(dir, name, AtFlags::empty()).map_err(map)
            }
            Err(Errno::NOENT) => Ok(()),
            _ => Err(WorktreeError::Io),
        }
    }
    /// Error-path removal of OUR temp: returns `err` when the temp is gone, or
    /// `Io` when its name now holds a foreign entry (left in place).
    fn discard(dir: &OwnedFd, tmp: &Temp, err: WorktreeError) -> WorktreeError {
        match unlink_owned(dir, &tmp.name, tmp.owned) {
            Ok(()) => err,
            Err(_) => WorktreeError::Io,
        }
    }
    pub(super) fn sync(dir: &OwnedFd) -> Result<(), WorktreeError> {
        fsync(dir).map_err(map)
    }
    fn rename2(dir: &OwnedFd, from: &str, to: &str, flags: RenameFlags) -> rustix::io::Result<()> {
        #[cfg(test)]
        if FORCE_PLAIN_RENAME.get() {
            return Err(Errno::INVAL);
        }
        renameat_with(dir, from, dir, to, flags)
    }
    fn unsupported(e: Errno) -> bool {
        e == Errno::INVAL || e == Errno::NOSYS || e == Errno::OPNOTSUPP
    }
    /// Expected absent: RENAME_NOREPLACE, so a racing creator is never clobbered.
    /// On any error the temp is removed only if it is still our inode.
    pub(super) fn create_noreplace(
        dir: &OwnedFd,
        tmp: &Temp,
        name: &str,
        rel: &str,
    ) -> Result<(), WorktreeError> {
        hook("commit", &tmp.name);
        create_noreplace_inner(dir, &tmp.name, name, rel).map_err(|e| discard(dir, tmp, e))
    }
    fn create_noreplace_inner(
        dir: &OwnedFd,
        tmp: &str,
        name: &str,
        rel: &str,
    ) -> Result<(), WorktreeError> {
        let mismatch = || WorktreeError::PreImageMismatch {
            path: rel.to_owned(),
        };
        match rename2(dir, tmp, name, RenameFlags::NOREPLACE) {
            Ok(()) => Ok(()),
            Err(Errno::EXIST) => Err(mismatch()),
            // Residual fallback (no renameat2 on this filesystem): check-then-rename.
            Err(e) if unsupported(e) => match openat(
                dir,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Err(Errno::NOENT) => renameat(dir, tmp, dir, name).map_err(map),
                Err(Errno::LOOP) => Err(WorktreeError::Symlink),
                _ => Err(mismatch()),
            },
            Err(e) => Err(map(e)),
        }
    }
    /// Expected present: atomically EXCHANGE temp and target, then require the
    /// displaced inode (now at the temp name) to be exactly the expected regular
    /// pre-image (then unlink exactly that inode); otherwise exchange back and
    /// fail, removing the temp name only if it is our inode again.
    pub(super) fn exchange_verified(
        dir: &OwnedFd,
        tmp: &Temp,
        name: &str,
        dev: u64,
        expected: &str,
        rel: &str,
    ) -> Result<(), WorktreeError> {
        let mismatch = || WorktreeError::PreImageMismatch {
            path: rel.to_owned(),
        };
        hook("commit", &tmp.name);
        match rename2(dir, &tmp.name, name, RenameFlags::EXCHANGE) {
            Ok(()) => {}
            Err(Errno::NOENT) => return Err(discard(dir, tmp, mismatch())),
            Err(e) if unsupported(e) => {
                // Residual fallback: re-check, then plain atomic rename.
                if let Err(e) = check_pre(dir, name, dev, Some(expected), rel) {
                    return Err(discard(dir, tmp, e));
                }
                return renameat(dir, tmp.name.as_str(), dir, name)
                    .map_err(|e| discard(dir, tmp, map(e)));
            }
            Err(e) => return Err(discard(dir, tmp, map(e))),
        }
        // `tmp.name` now names the DISPLACED entry.
        match read_owned(dir, &tmp.name, dev) {
            Ok(Some((old, displaced))) if hash(&old) == expected => {
                unlink_owned(dir, &tmp.name, displaced)
            }
            other => {
                hook("restore", &tmp.name);
                // Put the displaced entry back; our bytes return to `tmp`. If
                // that fails, `tmp` keeps the displaced entry: never unlink it.
                renameat_with(dir, tmp.name.as_str(), dir, name, RenameFlags::EXCHANGE)
                    .map_err(|_| WorktreeError::Io)?;
                let err = match other {
                    Err(WorktreeError::Symlink) => WorktreeError::Symlink,
                    Err(WorktreeError::NotRegular) => WorktreeError::NotRegular,
                    _ => mismatch(),
                };
                // A concurrent writer may have replaced `name` in between; the
                // temp name then holds THEIR entry, which is kept (`Io`).
                Err(discard(dir, tmp, err))
            }
        }
    }
    /// Rename the target aside (NOREPLACE), verify the moved inode is exactly the
    /// expected regular pre-image, then unlink it; otherwise move it back.
    pub(super) fn remove_verified(
        dir: &OwnedFd,
        name: &str,
        dev: u64,
        expected: &str,
        rel: &str,
    ) -> Result<(), WorktreeError> {
        let mismatch = || WorktreeError::PreImageMismatch {
            path: rel.to_owned(),
        };
        let aside = format!(".pai-del-{}", nonce().map_err(|_| WorktreeError::Io)?);
        hook("commit", &aside);
        match rename2(dir, name, &aside, RenameFlags::NOREPLACE) {
            Ok(()) => {}
            Err(Errno::NOENT) => return Err(mismatch()),
            Err(e) if unsupported(e) => renameat(dir, name, dir, aside.as_str()).map_err(map)?,
            Err(e) => return Err(map(e)),
        }
        match read_owned(dir, &aside, dev) {
            Ok(Some((old, moved))) if hash(&old) == expected => unlink_owned(dir, &aside, moved),
            other => {
                hook("restore", &aside);
                // NOREPLACE: a concurrent creator of `name` is never clobbered;
                // the moved entry then stays at `aside` (`Io`), never deleted.
                match rename2(dir, &aside, name, RenameFlags::NOREPLACE) {
                    Ok(()) => {}
                    Err(e) if unsupported(e) => {
                        renameat(dir, aside.as_str(), dir, name).map_err(map)?;
                    }
                    Err(_) => return Err(WorktreeError::Io),
                }
                Err(match other {
                    Err(WorktreeError::Symlink) => WorktreeError::Symlink,
                    Err(WorktreeError::NotRegular) => WorktreeError::NotRegular,
                    _ => mismatch(),
                })
            }
        }
    }
}

/// In-memory `WorktreeIo` for orchestrator/unit tests (counts writes).
#[cfg(test)]
pub(crate) struct MemoryWorktree {
    binding: WorktreeBinding,
    pub(crate) files: std::sync::Mutex<BTreeMap<String, Vec<u8>>>,
    pub(crate) writes: std::sync::atomic::AtomicU64,
}
#[cfg(test)]
impl MemoryWorktree {
    pub(crate) fn new() -> Self {
        Self {
            binding: WorktreeBinding {
                path: "/memory/pai-task-00000000".into(),
                dev: 0,
                ino: 0,
                created_at_ms: 0,
            },
            files: Default::default(),
            writes: Default::default(),
        }
    }
}
#[cfg(test)]
impl WorktreeIo for MemoryWorktree {
    fn binding(&self) -> &WorktreeBinding {
        &self.binding
    }
    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>, WorktreeError> {
        split(rel)?;
        Ok(self.files.lock().unwrap().get(rel).cloned())
    }
    fn replace_atomic(
        &self,
        rel: &str,
        bytes: &[u8],
        pre: Option<&str>,
    ) -> Result<(), WorktreeError> {
        split(rel)?;
        let mut files = self.files.lock().unwrap();
        if files.get(rel).map(|b| hash(b)).as_deref() != pre {
            return Err(WorktreeError::PreImageMismatch { path: rel.into() });
        }
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        files.insert(rel.into(), bytes.to_vec());
        Ok(())
    }
    fn remove(&self, rel: &str, pre: &str) -> Result<(), WorktreeError> {
        split(rel)?;
        let mut files = self.files.lock().unwrap();
        if files.get(rel).map(|b| hash(b)).as_deref() != Some(pre) {
            return Err(WorktreeError::PreImageMismatch { path: rel.into() });
        }
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        files.remove(rel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn tid(n: u32) -> TaskId {
        TaskId::parse(&format!("{n:08x}{}", "0".repeat(24))).unwrap()
    }
    fn post(entries: &[(&str, Option<&[u8]>)]) -> BTreeMap<String, Option<Vec<u8>>> {
        entries
            .iter()
            .map(|(p, b)| ((*p).to_owned(), b.map(<[u8]>::to_vec)))
            .collect()
    }

    #[test]
    fn task_worktree_apply_changes_checks_every_pre_image_before_any_write() {
        let mem = MemoryWorktree::new();
        let first = plan_changes(
            &BTreeMap::new(),
            &post(&[("a.py", Some(b"a1")), ("d/b.py", Some(b"b1"))]),
        );
        assert_eq!(apply_changes(&mem, &first).unwrap(), vec!["a.py", "d/b.py"]);
        let writes = mem.writes.load(Ordering::SeqCst);
        let mut bad = plan_changes(
            &observe(&mem, &["a.py".into(), "d/b.py".into()]).unwrap(),
            &post(&[("a.py", Some(b"a2")), ("d/b.py", None)]),
        );
        bad[1].expected_pre = Some(hash(b"not what is there"));
        assert_eq!(
            apply_changes(&mem, &bad),
            Err(WorktreeError::PreImageMismatch {
                path: "d/b.py".into()
            })
        );
        assert_eq!(
            mem.writes.load(Ordering::SeqCst),
            writes,
            "no write before all pre-images match"
        );
        let dup = vec![first[0].clone(), first[0].clone()];
        assert_eq!(
            apply_changes(&mem, &dup),
            Err(WorktreeError::DeniedLocation)
        );
        for path in ["../x", "/abs", ".git/config", "a//b"] {
            let c = WorktreeChange {
                path: path.into(),
                expected_pre: None,
                post: Some(b"x".to_vec()),
            };
            assert_eq!(
                apply_changes(&mem, &[c]),
                Err(WorktreeError::DeniedLocation),
                "{path}"
            );
        }
        assert_eq!(mem.writes.load(Ordering::SeqCst), writes);
        let b = serde_json::to_string(mem.binding()).unwrap();
        assert!(!b.to_ascii_lowercase().contains("pid"));
        assert!(
            serde_json::from_str::<WorktreeBinding>(&b.replacen('{', r#"{"pid":1,"#, 1)).is_err()
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn task_worktree_non_linux_is_unavailable() {
        let policy = WorktreePolicy {
            base_dir: std::env::temp_dir(),
            deny_within: vec![],
        };
        assert_eq!(
            LinuxWorktree::create(&policy, &tid(1)).err(),
            Some(WorktreeError::Unavailable)
        );
        let b = WorktreeBinding {
            path: "x".into(),
            dev: 0,
            ino: 0,
            created_at_ms: 0,
        };
        assert_eq!(
            LinuxWorktree::reopen(&policy, &b).err(),
            Some(WorktreeError::Unavailable)
        );
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;
        use std::fs;
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        use std::path::Path;
        use std::sync::atomic::{AtomicBool, AtomicU64};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        const OUTSIDE: &[u8] = b"SYNTHETIC_OUTSIDE_WORKTREE_CONTENT";

        struct Fx {
            t: tempfile::TempDir,
            base: PathBuf,
            outside: PathBuf,
            policy: WorktreePolicy,
        }
        fn fixture() -> Fx {
            let t = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(t.path()).unwrap();
            let base = root.join("coding-tasks");
            let outside = root.join("victim");
            for d in [&base, &outside, &root.join("vault"), &root.join("source")] {
                fs::create_dir(d).unwrap();
            }
            fs::write(outside.join("payload.txt"), OUTSIDE).unwrap();
            let policy = WorktreePolicy {
                base_dir: base.clone(),
                deny_within: vec![root.join("vault"), root.join("source")],
            };
            Fx {
                t,
                base,
                outside,
                policy,
            }
        }
        fn outside_intact(outside: &Path) -> bool {
            let names: Vec<_> = fs::read_dir(outside)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.len() == 1
                && names[0] == "payload.txt"
                && fs::read(outside.join("payload.txt")).unwrap() == OUTSIDE
        }
        /// Every regular file below `dir` (relative path → bytes), symlinks noted.
        fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
            let mut out = BTreeMap::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(&d).unwrap() {
                    let e = e.unwrap();
                    let p = e.path();
                    let rel = p.strip_prefix(dir).unwrap().to_str().unwrap().to_owned();
                    let ft = e.file_type().unwrap();
                    if ft.is_dir() {
                        stack.push(p);
                    } else if ft.is_symlink() {
                        out.insert(rel, b"<symlink>".to_vec());
                    } else if !ft.is_file() {
                        out.insert(rel, b"<special>".to_vec());
                    } else {
                        out.insert(rel, fs::read(&p).unwrap());
                    }
                }
            }
            out
        }

        #[test]
        fn task_worktree_apply_atomic_with_pre_image_check() {
            let fx = fixture();
            let wt = LinuxWorktree::create(&fx.policy, &tid(0x1234abcd)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            assert_eq!(root, fx.base.join("pai-task-1234abcd"));
            let meta = fs::metadata(&root).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
            assert_eq!(
                (meta.dev(), meta.ino()),
                (wt.binding().dev, wt.binding().ino)
            );
            let initial = post(&[
                ("src/a.py", Some(b"a = 1\n")),
                ("src/pkg/b.py", Some(b"b = 1\n")),
                ("README.md", Some(b"# r\n")),
            ]);
            let applied = apply_changes(&wt, &plan_changes(&BTreeMap::new(), &initial)).unwrap();
            assert_eq!(applied.len(), 3);
            assert_eq!(
                tree(&root),
                initial
                    .iter()
                    .map(|(p, b)| (p.clone(), b.clone().unwrap()))
                    .collect()
            );
            assert_eq!(
                fs::metadata(root.join("src/a.py"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(root.join("src/pkg"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            // Second apply: replace, remove, create — against observed pre-images.
            let pre = observe(&wt, &initial.keys().cloned().collect::<Vec<_>>()).unwrap();
            let next = post(&[
                ("src/a.py", Some(b"a = 2\n")),
                ("src/pkg/b.py", None),
                ("README.md", Some(b"# r\n")),
                ("src/c.py", Some(b"c\n")),
            ]);
            let changes = plan_changes(&pre, &next);
            assert_eq!(
                changes.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
                vec!["src/a.py", "src/c.py", "src/pkg/b.py"]
            );
            apply_changes(&wt, &changes).unwrap();
            assert_eq!(
                wt.read("src/a.py").unwrap().as_deref(),
                Some(b"a = 2\n".as_slice())
            );
            assert_eq!(wt.read("src/pkg/b.py").unwrap(), None);
            assert_eq!(
                wt.read("src/c.py").unwrap().as_deref(),
                Some(b"c\n".as_slice())
            );
            assert!(
                tree(&root).keys().all(|k| !k.contains(".pai-")),
                "no temp files left"
            );
            // Readers see old or new, never partial or missing (atomic exchange).
            let stop = Arc::new(AtomicBool::new(false));
            let seen_bad = Arc::new(AtomicU64::new(0));
            let reads = Arc::new(AtomicU64::new(0));
            let (s, bad, n, target) = (
                stop.clone(),
                seen_bad.clone(),
                reads.clone(),
                root.join("src/a.py"),
            );
            let reader = std::thread::spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    match fs::read(&target) {
                        Ok(b) if b.starts_with(b"version ") && b.len() == 64 => {}
                        Ok(b) if b == b"a = 2\n" => {}
                        _ => {
                            bad.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    n.fetch_add(1, Ordering::Relaxed);
                }
            });
            let mut prev = b"a = 2\n".to_vec();
            for i in 0..400 {
                let mut next = format!("version {i}").into_bytes();
                next.resize(64, b'.');
                wt.replace_atomic("src/a.py", &next, Some(&hash(&prev)))
                    .unwrap();
                prev = next;
            }
            stop.store(true, Ordering::SeqCst);
            reader.join().unwrap();
            println!(
                "STAGE5_B_ATOMIC reads={} bad={}",
                reads.load(Ordering::Relaxed),
                seen_bad.load(Ordering::Relaxed)
            );
            assert_eq!(seen_bad.load(Ordering::Relaxed), 0);
            assert!(reads.load(Ordering::Relaxed) > 0);
            // Reopen sees the same identity and content.
            let again = LinuxWorktree::reopen(&fx.policy, wt.binding()).unwrap();
            assert_eq!(again.read("src/a.py").unwrap(), Some(prev.clone()));
            // Fallback path (filesystem without renameat2): same semantics.
            fdops::FORCE_PLAIN_RENAME.set(true);
            let fallback = (|| {
                wt.replace_atomic("src/a.py", b"fallback\n", Some(&hash(&prev)))?;
                wt.replace_atomic("src/new.py", b"n\n", None)?;
                let mismatch = wt.replace_atomic("src/new.py", b"n2\n", None);
                wt.remove("src/new.py", &hash(b"n\n"))?;
                Ok::<_, WorktreeError>(mismatch)
            })();
            fdops::FORCE_PLAIN_RENAME.set(false);
            assert_eq!(
                fallback,
                Ok(Err(WorktreeError::PreImageMismatch {
                    path: "src/new.py".into()
                }))
            );
            assert_eq!(
                wt.read("src/a.py").unwrap().as_deref(),
                Some(b"fallback\n".as_slice())
            );
            assert_eq!(wt.read("src/new.py").unwrap(), None);
            assert!(tree(&root).keys().all(|k| !k.contains(".pai-")));
            assert!(outside_intact(&fx.outside));
        }

        #[test]
        fn task_worktree_pre_image_mismatch_denied_no_write() {
            let fx = fixture();
            let wt = LinuxWorktree::create(&fx.policy, &tid(2)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            apply_changes(
                &wt,
                &plan_changes(
                    &BTreeMap::new(),
                    &post(&[
                        ("a.py", Some(b"a\n")),
                        ("b.py", Some(b"b\n")),
                        ("d/c.py", Some(b"c\n")),
                    ]),
                ),
            )
            .unwrap();
            let snapshot = tree(&root);
            // One wrong pre-image among several: nothing is written.
            let mut changes = plan_changes(
                &observe(&wt, &["a.py".into(), "b.py".into(), "d/c.py".into()]).unwrap(),
                &post(&[
                    ("a.py", Some(b"A\n")),
                    ("b.py", None),
                    ("d/c.py", Some(b"C\n")),
                ]),
            );
            changes[2].expected_pre = Some(hash(b"stale"));
            assert_eq!(
                apply_changes(&wt, &changes),
                Err(WorktreeError::PreImageMismatch {
                    path: "d/c.py".into()
                })
            );
            assert_eq!(tree(&root), snapshot);
            let mismatch = |p: &str| Err(WorktreeError::PreImageMismatch { path: p.into() });
            assert_eq!(
                wt.replace_atomic("a.py", b"A\n", None),
                mismatch("a.py"),
                "expected absent but present"
            );
            assert_eq!(
                wt.replace_atomic("zzz.py", b"Z\n", Some(&hash(b"x"))),
                mismatch("zzz.py"),
                "expected present but absent"
            );
            assert_eq!(
                wt.replace_atomic("a.py", b"A\n", Some(&hash(b"wrong"))),
                mismatch("a.py")
            );
            assert_eq!(wt.remove("a.py", &hash(b"wrong")), mismatch("a.py"));
            assert_eq!(wt.remove("nope/x.py", &hash(b"x")), mismatch("nope/x.py"));
            assert_eq!(tree(&root), snapshot, "no write, no temp files");
            // Final-component symlink to an outside file: never followed.
            symlink(fx.outside.join("payload.txt"), root.join("link.txt")).unwrap();
            assert_eq!(wt.read("link.txt"), Err(WorktreeError::Symlink));
            assert_eq!(
                wt.replace_atomic("link.txt", b"PWNED", Some(&hash(OUTSIDE))),
                Err(WorktreeError::Symlink)
            );
            assert_eq!(
                wt.replace_atomic("link.txt", b"PWNED", None),
                Err(WorktreeError::Symlink)
            );
            assert_eq!(
                wt.remove("link.txt", &hash(OUTSIDE)),
                Err(WorktreeError::Symlink)
            );
            assert!(fs::symlink_metadata(root.join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink());
            // Directory-component symlink to an outside directory.
            symlink(&fx.outside, root.join("lnk")).unwrap();
            assert_eq!(wt.read("lnk/payload.txt"), Err(WorktreeError::Symlink));
            assert_eq!(
                wt.replace_atomic("lnk/payload.txt", b"PWNED", Some(&hash(OUTSIDE))),
                Err(WorktreeError::Symlink)
            );
            assert_eq!(
                wt.replace_atomic("lnk/new.txt", b"PWNED", None),
                Err(WorktreeError::Symlink)
            );
            assert_eq!(
                wt.remove("lnk/payload.txt", &hash(OUTSIDE)),
                Err(WorktreeError::Symlink)
            );
            // Hard links (inside or to an outside inode) and FIFOs are not regular.
            fs::hard_link(root.join("b.py"), root.join("b-alias.py")).unwrap();
            assert_eq!(wt.read("b.py"), Err(WorktreeError::NotRegular));
            assert_eq!(
                wt.replace_atomic("b.py", b"B\n", Some(&hash(b"b\n"))),
                Err(WorktreeError::NotRegular)
            );
            fs::remove_file(root.join("b-alias.py")).unwrap();
            fs::hard_link(fx.outside.join("payload.txt"), root.join("hl.txt")).unwrap();
            assert_eq!(wt.read("hl.txt"), Err(WorktreeError::NotRegular));
            assert_eq!(
                wt.replace_atomic("hl.txt", b"PWNED", Some(&hash(OUTSIDE))),
                Err(WorktreeError::NotRegular)
            );
            assert_eq!(
                wt.remove("hl.txt", &hash(OUTSIDE)),
                Err(WorktreeError::NotRegular)
            );
            fs::remove_file(root.join("hl.txt")).unwrap();
            rustix::fs::mknodat(
                rustix::fs::CWD,
                root.join("fifo"),
                rustix::fs::FileType::Fifo,
                rustix::fs::Mode::from_raw_mode(0o600),
                0,
            )
            .unwrap();
            let started = Instant::now();
            assert_eq!(wt.read("fifo"), Err(WorktreeError::NotRegular));
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "FIFO never blocks"
            );
            fs::create_dir(root.join("adir")).unwrap();
            assert_eq!(
                wt.replace_atomic("adir", b"x", Some(&hash(b"x"))),
                Err(WorktreeError::NotRegular)
            );
            // Invalid names never reach the filesystem.
            for p in [
                "../victim/payload.txt",
                "/etc/passwd",
                ".git/config",
                "a/../b.py",
                "",
            ] {
                assert_eq!(
                    wt.replace_atomic(p, b"x", None),
                    Err(WorktreeError::DeniedLocation),
                    "{p}"
                );
                assert_eq!(wt.read(p), Err(WorktreeError::DeniedLocation), "{p}");
            }
            assert!(outside_intact(&fx.outside));
            assert_eq!(fs::read(root.join("a.py")).unwrap(), b"a\n");
            assert!(tree(&root).keys().all(|k| !k.contains(".pai-")));
        }

        /// A concurrent same-UID writer: atomically renames fresh bytes onto `path`.
        fn clobber(path: &Path, bytes: &[u8]) {
            let staging = path.with_file_name("zz-concurrent-staging");
            fs::write(&staging, bytes).unwrap();
            fs::rename(&staging, path).unwrap();
        }
        /// Runs `op` with the fdops hook installed on THIS thread only; returns
        /// the result and the ordered (point, name) log.
        fn hooked<T>(
            mut on: impl FnMut(&'static str, &str) + 'static,
            op: impl FnOnce() -> T,
        ) -> (T, Vec<(String, String)>) {
            let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let l = log.clone();
            fdops::HOOK.set(Some(Box::new(move |point: &'static str, name: &str| {
                l.borrow_mut().push((point.to_owned(), name.to_owned()));
                on(point, name);
            })));
            let out = op();
            fdops::HOOK.set(None);
            let log = log.borrow().clone();
            (out, log)
        }
        fn name_at(log: &[(String, String)], point: &str) -> String {
            log.iter()
                .find(|(p, _)| p == point)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| panic!("hook point {point} not reached: {log:?}"))
        }

        /// Cleanup ownership (exchange, create and remove paths): a name is
        /// unlinked only while it is still the exact inode the helper owns. A
        /// concurrent writer's entry found under a temp/aside name is kept and
        /// the call reports `Io`; it is never deleted. Deterministic: the
        /// interference is injected at fixed points by a thread-local hook.
        #[test]
        fn task_worktree_cleanup_only_unlinks_owned_inodes() {
            let fx = fixture();
            let wt = LinuxWorktree::create(&fx.policy, &tid(0xc1ea)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            let initial: Vec<(String, Vec<u8>)> = (1..=8)
                .map(|i| (format!("d/f{i}.py"), format!("v0 {i}\n").into_bytes()))
                .collect();
            let pre = |i: usize| hash(&initial[i - 1].1);
            apply_changes(
                &wt,
                &plan_changes(
                    &BTreeMap::new(),
                    &initial
                        .iter()
                        .map(|(p, b)| (p.clone(), Some(b.clone())))
                        .collect(),
                ),
            )
            .unwrap();
            let d = root.join("d");
            let at = |name: &str| fs::read(d.join(name)).ok();
            let io = Err(WorktreeError::Io);
            let mismatch = |p: &str| Err(WorktreeError::PreImageMismatch { path: p.into() });

            // 1. Exchange succeeded and the displaced pre-image was verified, but
            //    before its unlink the temp name is replaced by a foreign file:
            //    the write stands, the foreign file is KEPT, the call says Io.
            let dd = d.clone();
            let (r, log) = hooked(
                move |point, name| {
                    if point == "cleanup" {
                        clobber(&dd.join(name), b"INTRUDER-1");
                    }
                },
                || wt.replace_atomic("d/f1.py", b"new 1\n", Some(&pre(1))),
            );
            assert_eq!(r, io.clone(), "{log:?}");
            assert_eq!(at("f1.py").as_deref(), Some(b"new 1\n".as_slice()));
            let t1 = name_at(&log, "cleanup");
            assert!(t1.starts_with(".pai-tmp-"), "{t1}");
            assert_eq!(at(&t1).as_deref(), Some(b"INTRUDER-1".as_slice()));
            fs::remove_file(d.join(&t1)).unwrap();

            // 1b. The displaced entry vanished before cleanup: nothing is left,
            //     so the replace succeeds (absent == cleaned).
            let dd = d.clone();
            let (r, _) = hooked(
                move |point, name| {
                    if point == "cleanup" {
                        fs::remove_file(dd.join(name)).unwrap();
                    }
                },
                || wt.replace_atomic("d/f2.py", b"new 2\n", Some(&pre(2))),
            );
            assert_eq!(r, Ok(()));
            assert_eq!(at("f2.py").as_deref(), Some(b"new 2\n".as_slice()));

            // 2. Restore path: writer #1 changes the target just before the
            //    exchange (so the displaced inode fails verification); writer #2
            //    replaces the target again just before the exchange-back. The
            //    temp name then holds writer #2's file: it must survive.
            let dd = d.clone();
            let (r, log) = hooked(
                move |point, _| match point {
                    "commit" => clobber(&dd.join("f3.py"), b"WRITER-1\n"),
                    "restore" => clobber(&dd.join("f3.py"), b"WRITER-2\n"),
                    _ => {}
                },
                || wt.replace_atomic("d/f3.py", b"new 3\n", Some(&pre(3))),
            );
            assert_eq!(r, io.clone(), "{log:?}");
            let t2 = name_at(&log, "restore");
            assert_eq!(name_at(&log, "cleanup"), t2);
            assert_eq!(at("f3.py").as_deref(), Some(b"WRITER-1\n".as_slice()));
            assert_eq!(
                at(&t2).as_deref(),
                Some(b"WRITER-2\n".as_slice()),
                "a concurrent writer's file is never deleted by cleanup"
            );
            fs::remove_file(d.join(&t2)).unwrap();

            // 2b. Control: only writer #1. Displaced entry restored, OUR temp
            //     (still our inode) removed, precise mismatch, no leftovers.
            let dd = d.clone();
            let (r, log) = hooked(
                move |point, _| {
                    if point == "commit" {
                        clobber(&dd.join("f4.py"), b"WRITER-1\n");
                    }
                },
                || wt.replace_atomic("d/f4.py", b"new 4\n", Some(&pre(4))),
            );
            assert_eq!(r, mismatch("d/f4.py"));
            assert_eq!(
                log.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
                vec!["commit", "restore", "cleanup"]
            );
            assert_eq!(at("f4.py").as_deref(), Some(b"WRITER-1\n".as_slice()));
            assert_eq!(at(&name_at(&log, "cleanup")), None);

            // 2c. Same interference on the no-renameat2 fallback: re-check fails,
            //     our temp is removed, the writer's content stands.
            let dd = d.clone();
            fdops::FORCE_PLAIN_RENAME.set(true);
            let (r, log) = hooked(
                move |point, _| {
                    if point == "commit" {
                        clobber(&dd.join("f5.py"), b"WRITER-1\n");
                    }
                },
                || wt.replace_atomic("d/f5.py", b"new 5\n", Some(&pre(5))),
            );
            fdops::FORCE_PLAIN_RENAME.set(false);
            assert_eq!(r, mismatch("d/f5.py"));
            assert_eq!(at("f5.py").as_deref(), Some(b"WRITER-1\n".as_slice()));
            assert_eq!(at(&name_at(&log, "cleanup")), None);

            // 3. Create (expected absent): a concurrent creator wins the name AND
            //    our temp name is replaced by a foreign file. NOREPLACE never
            //    clobbers the creator; the foreign temp-named file is kept (Io).
            let dd = d.clone();
            let (r, log) = hooked(
                move |point, name| {
                    if point == "commit" {
                        clobber(&dd.join("n1.py"), b"THEIRS\n");
                        clobber(&dd.join(name), b"INTRUDER-3");
                    }
                },
                || wt.replace_atomic("d/n1.py", b"ours\n", None),
            );
            assert_eq!(r, io.clone(), "{log:?}");
            assert_eq!(at("n1.py").as_deref(), Some(b"THEIRS\n".as_slice()));
            let t3 = name_at(&log, "commit");
            assert_eq!(at(&t3).as_deref(), Some(b"INTRUDER-3".as_slice()));
            fs::remove_file(d.join(&t3)).unwrap();
            // 3b. Control: creator only — precise mismatch, our temp removed.
            let dd = d.clone();
            let (r, log) = hooked(
                move |point, _| {
                    if point == "commit" {
                        clobber(&dd.join("n2.py"), b"THEIRS\n");
                    }
                },
                || wt.replace_atomic("d/n2.py", b"ours\n", None),
            );
            assert_eq!(r, mismatch("d/n2.py"));
            assert_eq!(at("n2.py").as_deref(), Some(b"THEIRS\n".as_slice()));
            assert_eq!(at(&name_at(&log, "commit")), None);

            // 4. Remove: target moved aside and verified, but the aside name is
            //    replaced before the unlink — the foreign file is kept (Io).
            let dd = d.clone();
            let (r, log) = hooked(
                move |point, name| {
                    if point == "cleanup" {
                        clobber(&dd.join(name), b"INTRUDER-4");
                    }
                },
                || wt.remove("d/f6.py", &pre(6)),
            );
            assert_eq!(r, io.clone(), "{log:?}");
            let t4 = name_at(&log, "cleanup");
            assert!(t4.starts_with(".pai-del-"), "{t4}");
            assert_eq!(at("f6.py"), None);
            assert_eq!(at(&t4).as_deref(), Some(b"INTRUDER-4".as_slice()));
            fs::remove_file(d.join(&t4)).unwrap();
            // 4b. Remove restore path: writer #1 changes the target before it is
            //     moved aside (verification fails), writer #2 re-creates the
            //     name before the move-back. Both files survive (Io).
            let dd = d.clone();
            let (r, log) = hooked(
                move |point, _| match point {
                    "commit" => clobber(&dd.join("f7.py"), b"WRITER-1\n"),
                    "restore" => clobber(&dd.join("f7.py"), b"WRITER-2\n"),
                    _ => {}
                },
                || wt.remove("d/f7.py", &pre(7)),
            );
            assert_eq!(r, io.clone(), "{log:?}");
            let t5 = name_at(&log, "restore");
            assert_eq!(at("f7.py").as_deref(), Some(b"WRITER-2\n".as_slice()));
            assert_eq!(at(&t5).as_deref(), Some(b"WRITER-1\n".as_slice()));
            fs::remove_file(d.join(&t5)).unwrap();
            // 4c. Control: writer #1 only — moved back, precise mismatch.
            let dd = d.clone();
            let (r, _) = hooked(
                move |point, _| {
                    if point == "commit" {
                        clobber(&dd.join("f8.py"), b"WRITER-1\n");
                    }
                },
                || wt.remove("d/f8.py", &pre(8)),
            );
            assert_eq!(r, mismatch("d/f8.py"));
            assert_eq!(at("f8.py").as_deref(), Some(b"WRITER-1\n".as_slice()));

            // The hook is gone: normal operation, and nothing of ours is left.
            wt.replace_atomic("d/f8.py", b"final\n", Some(&hash(b"WRITER-1\n")))
                .unwrap();
            assert!(
                tree(&root)
                    .keys()
                    .all(|k| !k.contains(".pai-") && !k.contains("staging")),
                "{:?}",
                tree(&root).keys().collect::<Vec<_>>()
            );
            assert!(outside_intact(&fx.outside));
        }

        /// Concurrent atomic swapper (renameat2 EXCHANGE, so the name never
        /// disappears) vs. the worktree writer/reader for 5 s. Escape = any byte
        /// written to / read from outside the worktree. Never a pass by timeout:
        /// attempts, successful in-worktree writes and swaps are all required.
        fn swap_race(kind: &str) {
            use rustix::fs::{renameat_with, RenameFlags, CWD};
            let fx = fixture();
            let wt =
                LinuxWorktree::create(&fx.policy, &tid(0x00ace000 + kind.len() as u32)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            let allowed = b"allowed-worktree-content".to_vec();
            apply_changes(
                &wt,
                &plan_changes(
                    &BTreeMap::new(),
                    &post(&[("inner/payload.txt", Some(&allowed))]),
                ),
            )
            .unwrap();
            let (a, b) = match kind {
                "dir" => {
                    symlink(&fx.outside, root.join("inner-link")).unwrap();
                    (root.join("inner"), root.join("inner-link"))
                }
                "final" => {
                    symlink(
                        fx.outside.join("payload.txt"),
                        root.join("inner/payload.link"),
                    )
                    .unwrap();
                    (
                        root.join("inner/payload.txt"),
                        root.join("inner/payload.link"),
                    )
                }
                _ => {
                    symlink(&fx.outside, fx.base.join("root-link")).unwrap();
                    (root.clone(), fx.base.join("root-link"))
                }
            };
            let stop = Arc::new(AtomicBool::new(false));
            let swaps = Arc::new(AtomicU64::new(0));
            let (flag, n) = (stop.clone(), swaps.clone());
            let swapper = std::thread::spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    if renameat_with(CWD, &a, CWD, &b, RenameFlags::EXCHANGE).is_ok() {
                        n.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
            let start = Instant::now();
            let (mut attempts, mut writes, mut reads, mut creates, mut errors, mut escaped) =
                (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
            while start.elapsed() < Duration::from_secs(5) && escaped == 0 {
                attempts += 1;
                match attempts % 3 {
                    // Only bytes this test wrote may ever be read back.
                    0 => match wt.read("inner/payload.txt") {
                        Ok(Some(bytes)) if bytes == allowed || bytes.starts_with(b"write-") => {
                            reads += 1
                        }
                        Ok(Some(_)) => escaped += 1,
                        _ => errors += 1,
                    },
                    // Observe-then-replace with the observed pre-image (the
                    // orchestrator's pattern). Under concurrent renames a write
                    // may land and still report an error (post-image check by
                    // name); the state is then "unknown", never outside.
                    1 => match wt.read("inner/payload.txt") {
                        Ok(Some(pre)) if pre == allowed || pre.starts_with(b"write-") => {
                            let next = format!("write-{attempts}").into_bytes();
                            match wt.replace_atomic("inner/payload.txt", &next, Some(&hash(&pre))) {
                                Ok(()) => writes += 1,
                                Err(_) => errors += 1,
                            }
                        }
                        Ok(Some(_)) => escaped += 1,
                        _ => errors += 1,
                    },
                    _ => {
                        let name = format!("inner/new-{attempts}.txt");
                        match wt.replace_atomic(&name, b"created", None) {
                            Ok(()) => {
                                creates += 1;
                                if wt.remove(&name, &hash(b"created")).is_err() {
                                    errors += 1;
                                }
                            }
                            Err(_) => errors += 1,
                        }
                    }
                }
                if attempts % 64 == 0 && !outside_intact(&fx.outside) {
                    escaped += 1;
                }
            }
            stop.store(true, Ordering::SeqCst);
            swapper.join().unwrap();
            if !outside_intact(&fx.outside) {
                escaped += 1;
            }
            let swaps = swaps.load(Ordering::Relaxed);
            println!(
                "STAGE5_B_WORKTREE_RACE kind={kind} escaped={escaped} attempts={attempts} writes={writes} reads={reads} creates={creates} errors={errors} swaps={swaps} seconds={:.2}",
                start.elapsed().as_secs_f64()
            );
            assert!(swaps > 0 && attempts > 100, "race not exercised");
            assert!(
                writes > 0 && reads > 0,
                "legitimate in-worktree operations must still succeed"
            );
            assert_eq!(escaped, 0, "worktree wrote or read outside its root");
            drop(fx.t);
        }
        #[test]
        fn task_worktree_symlink_component_and_final_swap_races_fail_closed() {
            for kind in ["dir", "final", "root"] {
                swap_race(kind);
            }
        }

        /// Sorted entry names of a directory reached through a PINNED fd (so
        /// they are the names inside that exact inode, wherever it is linked).
        fn names_via(dir: &fs::File) -> Vec<String> {
            use std::os::fd::AsRawFd;
            let mut v: Vec<String> = fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
        /// A same-UID host process looping renameat2(a, b, RENAME_EXCHANGE)
        /// until `stop`; returns the number of successful exchanges.
        fn exchanger(
            a: PathBuf,
            b: PathBuf,
            stop: Arc<AtomicBool>,
        ) -> std::thread::JoinHandle<u64> {
            use rustix::fs::{renameat_with, RenameFlags, CWD};
            std::thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    if renameat_with(CWD, &a, CWD, &b, RenameFlags::EXCHANGE).is_ok() {
                        n += 1;
                    }
                }
                n
            })
        }
        /// Puts `a` back to the inode `ino` (one more exchange) if the
        /// attacker stopped in the swapped position.
        fn unswap(a: &Path, b: &Path, ino: u64) {
            use rustix::fs::{renameat_with, RenameFlags, CWD};
            if fs::symlink_metadata(a).unwrap().ino() != ino {
                renameat_with(CWD, a, CWD, b, RenameFlags::EXCHANGE).unwrap();
            }
            assert_eq!(fs::symlink_metadata(a).unwrap().ino(), ino);
        }
        fn create(path: &str, bytes: &[u8]) -> WorktreeChange {
            WorktreeChange {
                path: path.into(),
                expected_pre: None,
                post: Some(bytes.to_vec()),
            }
        }

        /// Review R2 F1 crisp reproducer (`w7b`, ported): a same-UID process
        /// loops renameat2(`<wt>/src`, `<foreign>/src`, RENAME_EXCHANGE) with a
        /// REAL directory on the same filesystem while the worktree applies
        /// `src/f.py`. `apply_changes` must never return `Ok` while its bytes
        /// sit in the foreign inode, and nothing may ever land in that inode.
        /// Layouts: `src` made by the worktree (legitimate writes must still
        /// succeed under the race), and `src` made out of band exactly as in the
        /// review (never part of the worktree, so it is never traversed).
        fn exchange_reproducer(layout: &str) {
            use rustix::fs::{unlinkat, AtFlags};
            use std::os::fd::AsRawFd;
            let fx = fixture();
            let wt = LinuxWorktree::create(&fx.policy, &tid(0x7b00 + layout.len() as u32)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            if layout == "worktree-made" {
                apply_changes(&wt, &[create("src/seed.py", b"seed\n")]).unwrap();
            } else {
                fs::create_dir(root.join("src")).unwrap();
            }
            // The foreign directory is newer than the worktree root (as in R2).
            let foreign = fx.outside.parent().unwrap().join("foreign");
            fs::create_dir_all(foreign.join("src")).unwrap();
            let (a, b) = (root.join("src"), foreign.join("src"));
            let wfd = fs::File::open(&a).unwrap();
            let ofd = fs::File::open(&b).unwrap();
            let (w_ino, o_ino) = (wfd.metadata().unwrap().ino(), ofd.metadata().unwrap().ino());
            let stop = Arc::new(AtomicBool::new(false));
            let attacker = exchanger(a.clone(), b.clone(), stop.clone());
            let start = Instant::now();
            let (mut attempts, mut oks, mut landed) = (0u64, 0u64, 0u64);
            let mut errors: BTreeMap<String, u64> = BTreeMap::new();
            let mut bad_ok = None;
            while start.elapsed() < Duration::from_secs(3) {
                attempts += 1;
                let body = format!("APP-WRITTEN-{attempts}\n").into_bytes();
                let r = apply_changes(&wt, &[create("src/f.py", &body)]);
                let in_foreign = names_via(&ofd);
                if !in_foreign.is_empty() {
                    landed += 1;
                }
                match &r {
                    Ok(_) => oks += 1,
                    Err(e) => *errors.entry(format!("{e:?}")).or_default() += 1,
                }
                if r.is_ok() && in_foreign.iter().any(|n| n == "f.py") {
                    let bytes = fs::read(format!("/proc/self/fd/{}/f.py", ofd.as_raw_fd())).ok();
                    if bytes.as_deref() == Some(&body[..]) {
                        bad_ok = Some(attempts);
                        break;
                    }
                }
                let _ = unlinkat(&wfd, "f.py", AtFlags::empty());
                for n in in_foreign {
                    let _ = unlinkat(&ofd, n.as_str(), AtFlags::empty());
                }
            }
            stop.store(true, Ordering::SeqCst);
            let swaps = attacker.join().unwrap();
            unswap(&a, &b, w_ino);
            assert_eq!(fs::symlink_metadata(&b).unwrap().ino(), o_ino);
            println!(
                "STAGE5_FIXB_W7B layout={layout} attempts={attempts} ok={oks} landed_in_foreign={landed} bad_ok={bad_ok:?} swaps={swaps} errors={errors:?} seconds={:.2}",
                start.elapsed().as_secs_f64()
            );
            assert_eq!(
                bad_ok, None,
                "apply_changes returned Ok while its bytes were in the foreign directory ({layout})"
            );
            assert_eq!(
                landed, 0,
                "a write landed in the foreign directory ({layout})"
            );
            assert!(swaps > 100 && attempts > 10, "race not exercised");
            assert!(names_via(&ofd).is_empty());
            let _ = unlinkat(&wfd, "f.py", AtFlags::empty());
            if layout == "worktree-made" {
                assert!(
                    oks > 0,
                    "legitimate writes must still succeed under the race"
                );
                // Control after the race: the write lands in the worktree's `src`.
                apply_changes(&wt, &[create("src/after.py", b"after\n")]).unwrap();
                assert_eq!(names_via(&wfd), vec!["after.py", "seed.py"]);
            } else {
                assert_eq!(oks, 0, "a directory the worktree never made is never used");
                assert!(errors.keys().all(|e| e == "IdentityChanged"), "{errors:?}");
                assert_eq!(
                    apply_changes(&wt, &[create("src/after.py", b"after\n")]),
                    Err(WorktreeError::IdentityChanged)
                );
                assert!(names_via(&wfd).is_empty());
            }
            assert!(names_via(&ofd).is_empty());
            assert!(outside_intact(&fx.outside));
        }
        #[test]
        fn task_worktree_rename_exchange_reproducer_never_ok_with_outside_bytes() {
            for layout in ["worktree-made", "out-of-band"] {
                exchange_reproducer(layout);
            }
        }

        /// Review R2 F1 5 s loop (`w7`, ported): create / replace / remove of
        /// `src/f.py` (through `replace_atomic`, `remove` and `apply_changes`)
        /// while `<wt>/src` (made by the worktree) is RENAME_EXCHANGEd with a real
        /// foreign directory. Escape = any entry ever appearing in the foreign
        /// inode (pinned fd; the attacker only renames). Never a pass by
        /// timeout: exchanges, attempts and successful writes are required.
        #[test]
        fn task_worktree_rename_exchange_with_foreign_directory_5s_never_writes_outside() {
            use rustix::fs::{unlinkat, AtFlags};
            let fx = fixture();
            let wt = LinuxWorktree::create(&fx.policy, &tid(0x00f7_0005)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            apply_changes(&wt, &[create("src/seed.py", b"seed\n")]).unwrap();
            let foreign = fx.outside.parent().unwrap().join("foreign");
            fs::create_dir_all(foreign.join("src")).unwrap();
            let (a, b) = (root.join("src"), foreign.join("src"));
            let w_ino = fs::metadata(&a).unwrap().ino();
            let ofd = fs::File::open(&b).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let attacker = exchanger(a.clone(), b.clone(), stop.clone());
            let mut errors: BTreeMap<String, u64> = BTreeMap::new();
            let (mut attempts, mut oks, mut escapes, mut landed_entries) = (0u64, 0u64, 0u64, 0u64);
            let mut first_escape = String::new();
            let mut have: Option<Vec<u8>> = None;
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                attempts += 1;
                let i = attempts;
                let body = format!("v{i}\n").into_bytes();
                let (op, r) = match &have {
                    None if i % 2 == 0 => ("create", wt.replace_atomic("src/f.py", &body, None)),
                    None => (
                        "create",
                        apply_changes(&wt, &[create("src/f.py", &body)]).map(|_| ()),
                    ),
                    Some(cur) if i % 3 == 0 => ("remove", wt.remove("src/f.py", &hash(cur))),
                    Some(cur) if i % 2 == 0 => (
                        "replace",
                        wt.replace_atomic("src/f.py", &body, Some(&hash(cur))),
                    ),
                    Some(cur) => (
                        "replace",
                        apply_changes(
                            &wt,
                            &[WorktreeChange {
                                path: "src/f.py".into(),
                                expected_pre: Some(hash(cur)),
                                post: Some(body.clone()),
                            }],
                        )
                        .map(|_| ()),
                    ),
                };
                match &r {
                    Ok(()) => {
                        oks += 1;
                        have = (op != "remove").then(|| body.clone());
                    }
                    Err(e) => {
                        *errors.entry(format!("{op}:{e:?}")).or_default() += 1;
                        if let Ok(x) = wt.read("src/f.py") {
                            have = x;
                        }
                    }
                }
                let landed = names_via(&ofd);
                if !landed.is_empty() {
                    escapes += 1;
                    landed_entries += landed.len() as u64;
                    if first_escape.is_empty() {
                        first_escape =
                            format!("iteration {i} op {op} result {r:?} entries {landed:?}");
                    }
                    for n in &landed {
                        let _ = unlinkat(&ofd, n.as_str(), AtFlags::empty());
                    }
                }
            }
            stop.store(true, Ordering::SeqCst);
            let swaps = attacker.join().unwrap();
            unswap(&a, &b, w_ino);
            println!(
                "STAGE5_FIXB_W7 escapes={escapes} landed_entries={landed_entries} attempts={attempts} ok={oks} swaps={swaps} errors={errors:?} seconds={:.2}",
                start.elapsed().as_secs_f64()
            );
            assert!(swaps > 100 && attempts > 100, "race not exercised");
            assert!(oks > 0, "legitimate in-worktree writes must still succeed");
            assert_eq!(
                escapes, 0,
                "write landed in a foreign directory inode: {first_escape}"
            );
            assert!(names_via(&ofd).is_empty());
            // Layout restored: the worktree reads exactly what is in its own `src`.
            let now = wt.read("src/f.py").unwrap();
            assert_eq!(now, fs::read(root.join("src/f.py")).ok());
            assert!(outside_intact(&fx.outside));
        }

        fn xchg(a: &Path, b: &Path) {
            use rustix::fs::{renameat_with, RenameFlags, CWD};
            renameat_with(CWD, a, CWD, b, RenameFlags::EXCHANGE).unwrap();
        }
        /// Every directory below `dir` (relative paths), including empty ones.
        fn dirs_below(dir: &Path) -> Vec<String> {
            let mut out = Vec::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(&d).unwrap() {
                    let e = e.unwrap();
                    if e.file_type().unwrap().is_dir() {
                        let p = e.path();
                        out.push(p.strip_prefix(dir).unwrap().to_str().unwrap().to_owned());
                        stack.push(p);
                    }
                }
            }
            out.sort();
            out
        }

        fn ic<T>() -> Result<T, WorktreeError> {
            Err(WorktreeError::IdentityChanged)
        }
        /// Review R2 F1, deterministic: a directory is only ever used by the
        /// identity RECORDED when the worktree made it. (1) An exchanged-in
        /// foreign directory holding the same bytes is refused for every
        /// operation, also through `reopen`. (2) A recorded directory moved
        /// away between the walk and the write keeps the bytes (it is the
        /// worktree's) and the post-write re-check makes the call
        /// `IdentityChanged`, never `Ok`. (3) Directories made or re-created out
        /// of band are never used. (4) The mkdir window: a populated directory
        /// substituted at the temp name is refused, untouched. (5) Restart: the
        /// record is re-derived from kernel evidence; an older foreign directory
        /// is never adopted. (6) The no-renameat2 fallback publishes directories.
        #[test]
        fn task_worktree_directory_identity_pinned_and_rechecked_after_write() {
            let fx = fixture();
            let top = fx.outside.parent().unwrap().to_path_buf();
            // Older than the worktree root, 0700: only the birth time tells it apart.
            let old = top.join("old");
            fs::create_dir_all(old.join("src")).unwrap();
            fs::write(old.join("src/a.py"), b"a\n").unwrap();
            fs::set_permissions(old.join("src"), fs::Permissions::from_mode(0o700)).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            let wt = LinuxWorktree::create(&fx.policy, &tid(0x1d00)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            apply_changes(
                &wt,
                &plan_changes(
                    &BTreeMap::new(),
                    &post(&[("src/a.py", Some(b"a\n")), ("src/pkg/b.py", Some(b"b\n"))]),
                ),
            )
            .unwrap();
            for d in ["src", "src/pkg"] {
                let mode = fs::metadata(root.join(d)).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o700, "{d}");
            }
            assert_eq!(
                dirs_below(&root),
                vec!["src", "src/pkg"],
                "no .pai-dir-* left"
            );
            // Same bytes as the worktree: a pre-image check cannot tell them apart.
            let foreign = top.join("foreign");
            fs::create_dir_all(foreign.join("src/pkg")).unwrap();
            fs::create_dir_all(foreign.join("fresh")).unwrap();
            fs::write(foreign.join("src/a.py"), b"a\n").unwrap();
            fs::write(foreign.join("src/pkg/b.py"), b"b\n").unwrap();
            let foreign_before = tree(&foreign);
            let old_before = tree(&old);
            let (w, o) = (root.join("src"), foreign.join("src"));

            // (1) Exchanged in before the call.
            xchg(&w, &o);
            let reopened = LinuxWorktree::reopen(&fx.policy, wt.binding()).unwrap();
            for io in [&wt as &dyn WorktreeIo, &reopened] {
                assert_eq!(io.read("src/a.py"), ic());
                assert_eq!(io.read("src/pkg/b.py"), ic());
                assert_eq!(
                    io.replace_atomic("src/a.py", b"A\n", Some(&hash(b"a\n"))),
                    ic()
                );
                assert_eq!(io.replace_atomic("src/new.py", b"N\n", None), ic());
                assert_eq!(io.replace_atomic("src/pkg/new.py", b"N\n", None), ic());
                assert_eq!(io.replace_atomic("src/sub/new.py", b"N\n", None), ic());
                assert_eq!(io.remove("src/a.py", &hash(b"a\n")), ic());
                assert_eq!(apply_changes(io, &[create("src/n.py", b"n\n")]), ic());
                assert!(observe(io, &["src/a.py".into()]).is_err());
            }
            assert_eq!(
                tree(&foreign),
                foreign_before,
                "foreign directory never written"
            );
            xchg(&w, &o);
            assert_eq!(
                reopened.read("src/pkg/b.py").unwrap().as_deref(),
                Some(b"b\n".as_slice())
            );

            // (2) Moved away between the walk and the write (hook at "commit").
            let cases: [(&str, &str, &str); 5] = [
                ("create", "src", "src/c.py"),
                ("replace", "src", "src/a.py"),
                ("nested", "src", "src/pkg/d.py"),
                ("remove", "src", "src/c.py"),
                ("new-dir", "fresh", "fresh/x.py"),
            ];
            for (op, dir, path) in cases {
                let (a, b) = (root.join(dir), foreign.join(dir));
                let (aa, bb) = (a.clone(), b.clone());
                let mut armed = true;
                let (r, log) = hooked(
                    move |point, _| {
                        if point == "commit" && armed {
                            armed = false;
                            xchg(&aa, &bb);
                        }
                    },
                    || match op {
                        "replace" => wt.replace_atomic(path, b"A2\n", Some(&hash(b"a\n"))),
                        "remove" => wt.remove(path, &hash(b"C\n")),
                        _ => wt.replace_atomic(path, b"C\n", None),
                    },
                );
                assert_eq!(r, ic(), "{op}: {log:?}");
                assert!(log.iter().any(|(p, _)| p == "verify"), "{op}: {log:?}");
                xchg(&a, &b);
                let landed = fs::read(root.join(path)).ok();
                let expected: Option<&[u8]> = match op {
                    "replace" => Some(b"A2\n"),
                    "remove" => None,
                    _ => Some(b"C\n"),
                };
                assert_eq!(
                    landed.as_deref(),
                    expected,
                    "{op}: in the worktree's own inode"
                );
                assert_eq!(tree(&foreign), foreign_before, "{op}: foreign untouched");
            }
            // Control: no interference, plain success.
            wt.replace_atomic("src/a.py", b"A3\n", Some(&hash(b"A2\n")))
                .unwrap();

            // (3) Made or re-created out of band: never used (also after reopen).
            fs::create_dir(root.join("extra")).unwrap();
            fs::create_dir(root.join("extra700")).unwrap();
            fs::set_permissions(root.join("extra700"), fs::Permissions::from_mode(0o700)).unwrap();
            let pkg_aside = root.join("pkg-aside");
            fs::rename(root.join("src/pkg"), &pkg_aside).unwrap();
            fs::create_dir(root.join("src/pkg")).unwrap();
            fs::write(root.join("src/pkg/b.py"), b"b\n").unwrap();
            let again = LinuxWorktree::reopen(&fx.policy, wt.binding()).unwrap();
            for io in [&wt as &dyn WorktreeIo, &again] {
                assert_eq!(io.replace_atomic("extra/x.py", b"x\n", None), ic());
                assert_eq!(io.replace_atomic("extra700/x.py", b"x\n", None), ic());
                assert_eq!(io.read("extra/x.py"), ic());
                assert_eq!(io.read("src/pkg/b.py"), ic(), "re-created directory");
                assert_eq!(io.replace_atomic("src/pkg/e.py", b"e\n", None), ic());
            }
            assert!(fs::read_dir(root.join("extra")).unwrap().next().is_none());
            assert!(fs::read_dir(root.join("extra700"))
                .unwrap()
                .next()
                .is_none());
            fs::remove_dir_all(root.join("src/pkg")).unwrap();
            fs::rename(&pkg_aside, root.join("src/pkg")).unwrap();
            assert_eq!(
                wt.read("src/pkg/d.py").unwrap().as_deref(),
                Some(b"C\n".as_slice())
            );

            // (4) The mkdir window: a populated directory swapped in at the
            //     temp name before it is opened is refused and left untouched.
            let (r2, fo) = (root.clone(), old.join("src"));
            let (r, log) = hooked(
                move |point, name| {
                    if point == "mkdir" {
                        xchg(&r2.join(name), &fo);
                    }
                },
                || wt.replace_atomic("made/x.py", b"x\n", None),
            );
            assert_eq!(r, ic(), "{log:?}");
            let tmp = name_at(&log, "mkdir");
            assert!(tmp.starts_with(".pai-dir-"), "{tmp}");
            xchg(&root.join(&tmp), &old.join("src"));
            assert_eq!(tree(&old), old_before, "substituted directory untouched");
            assert!(!root.join("made").exists());
            fs::remove_dir(root.join(&tmp)).unwrap();

            // (5) Restart: no record in this process; reopen re-derives it.
            //     An older foreign directory exchanged in is never adopted.
            xchg(&w, &old.join("src"));
            registry::forget(wt.binding());
            let restarted = LinuxWorktree::reopen(&fx.policy, wt.binding()).unwrap();
            assert_eq!(restarted.read("src/a.py"), ic());
            assert_eq!(restarted.replace_atomic("src/z.py", b"z\n", None), ic());
            assert_eq!(restarted.remove("src/a.py", &hash(b"a\n")), ic());
            xchg(&w, &old.join("src"));
            assert_eq!(
                tree(&old),
                old_before,
                "older directory never adopted or written"
            );
            registry::forget(wt.binding());
            let restarted = LinuxWorktree::reopen(&fx.policy, wt.binding()).unwrap();
            assert_eq!(
                restarted.read("src/pkg/b.py").unwrap().as_deref(),
                Some(b"b\n".as_slice())
            );
            restarted
                .replace_atomic("fresh/x.py", b"X2\n", Some(&hash(b"C\n")))
                .unwrap();
            restarted
                .replace_atomic("src/pkg/q/r.py", b"r\n", None)
                .unwrap();
            // 0755 out-of-band directory: not adopted. A 0700 one made after the
            // root by the same user IS adopted (documented residual, module docs).
            assert_eq!(restarted.replace_atomic("extra/x.py", b"x\n", None), ic());
            restarted
                .replace_atomic("extra700/x.py", b"x\n", None)
                .unwrap();

            // (6) No-renameat2 fallback: new directories are still published.
            fdops::FORCE_PLAIN_RENAME.set(true);
            let plain = restarted.replace_atomic("plain/deep/x.py", b"p\n", None);
            fdops::FORCE_PLAIN_RENAME.set(false);
            assert_eq!(plain, Ok(()));
            assert_eq!(
                restarted.read("plain/deep/x.py").unwrap().as_deref(),
                Some(b"p\n".as_slice())
            );
            assert!(
                dirs_below(&root).iter().all(|d| !d.contains(".pai-")),
                "{:?}",
                dirs_below(&root)
            );
            assert!(tree(&root).keys().all(|k| !k.contains(".pai-")));
            assert_eq!(tree(&foreign), foreign_before);
            assert!(outside_intact(&fx.outside));
        }

        #[test]
        fn task_worktree_location_policy_denies_source_install_vault_scratch_home() {
            let fx = fixture();
            let top = fs::canonicalize(fx.t.path()).unwrap();
            for d in [
                "install",
                "scratch",
                "apps",
                "vault2",
                "source/sub",
                "vault/x",
                "install/data",
                "scratch/w",
            ] {
                fs::create_dir_all(top.join(d)).unwrap();
            }
            symlink(top.join("apps"), top.join("apps-link")).unwrap();
            symlink(&top, top.join("top-link")).unwrap();
            let deny = vec![
                top.join("source"),
                top.join("install"),
                top.join("vault"),
                top.join("scratch"),
            ];
            let at = |base: PathBuf, deny_within: Vec<PathBuf>| {
                LinuxWorktree::create(
                    &WorktreePolicy {
                        base_dir: base,
                        deny_within,
                    },
                    &tid(0xfeed),
                )
                .map(|w| w.binding().path.clone())
            };
            let denied = WorktreeError::DeniedLocation;
            for base in [
                top.join("source"),
                top.join("source/sub"),
                top.join("install"),
                top.join("install/data"),
                top.join("vault"),
                top.join("vault/x"),
                top.join("scratch"),
                top.join("scratch/w"),
                top.join("apps-link"),
                top.join("apps/../apps"),
                top.join("missing"),
                PathBuf::from("apps"),
                PathBuf::from("/"),
                PathBuf::from("/usr/lib"),
                PathBuf::from("/etc"),
                PathBuf::from("/proc"),
                PathBuf::from("/dev"),
                PathBuf::from("/home"),
            ] {
                assert_eq!(
                    at(base.clone(), deny.clone()),
                    Err(denied.clone()),
                    "{base:?}"
                );
            }
            if let Some(home) = std::env::var_os("HOME").and_then(|h| fs::canonicalize(h).ok()) {
                let before: Vec<_> = fs::read_dir(&home)
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                assert_eq!(
                    at(home.clone(), deny.clone()),
                    Err(denied.clone()),
                    "home itself"
                );
                let after: Vec<_> = fs::read_dir(&home)
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                assert_eq!(before.len(), after.len(), "nothing created in home");
            }
            // Deny entries through a symlink or relative are fail-closed.
            assert_eq!(
                at(top.join("apps"), vec![top.join("top-link")]),
                Err(denied.clone())
            );
            assert_eq!(
                at(top.join("apps"), vec![PathBuf::from("relative")]),
                Err(denied.clone())
            );
            // A deny entry that would live INSIDE the new worktree.
            assert_eq!(
                at(
                    top.join("apps"),
                    vec![top.join("apps/pai-task-0000feed/vault")]
                ),
                Err(denied.clone())
            );
            for d in ["source", "install", "vault", "scratch", "apps"] {
                assert!(
                    fs::read_dir(top.join(d)).unwrap().all(|e| !e
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("pai-task-")),
                    "{d}"
                );
            }
            // Controls: allowed, including the sibling-prefix name vault2.
            assert_eq!(
                at(top.join("apps"), deny.clone()),
                Ok(top.join("apps/pai-task-0000feed").to_str().unwrap().into())
            );
            assert_eq!(
                at(top.join("vault2"), deny.clone()),
                Ok(top
                    .join("vault2/pai-task-0000feed")
                    .to_str()
                    .unwrap()
                    .into())
            );
            // Never reuse: EEXIST is an error and the existing dir is untouched.
            fs::write(top.join("apps/pai-task-0000feed/keep.txt"), b"keep").unwrap();
            assert_eq!(at(top.join("apps"), deny), Err(denied));
            assert_eq!(
                fs::read(top.join("apps/pai-task-0000feed/keep.txt")).unwrap(),
                b"keep"
            );
        }

        #[test]
        fn task_worktree_reopen_identity_changed_denied() {
            let fx = fixture();
            let wt = LinuxWorktree::create(&fx.policy, &tid(0xbeef)).unwrap();
            apply_changes(
                &wt,
                &plan_changes(&BTreeMap::new(), &post(&[("f.py", Some(b"f\n"))])),
            )
            .unwrap();
            let binding = wt.binding().clone();
            drop(wt);
            let root = PathBuf::from(&binding.path);
            let old = fx.base.join("moved-away");
            assert_eq!(
                LinuxWorktree::reopen(&fx.policy, &binding)
                    .unwrap()
                    .read("f.py")
                    .unwrap()
                    .as_deref(),
                Some(b"f\n".as_slice())
            );
            let changed = Err(WorktreeError::IdentityChanged);
            fs::rename(&root, &old).unwrap();
            fs::create_dir(&root).unwrap();
            fs::write(root.join("f.py"), b"f\n").unwrap();
            assert_eq!(
                LinuxWorktree::reopen(&fx.policy, &binding).map(|_| ()),
                changed.clone(),
                "replaced directory"
            );
            fs::remove_dir_all(&root).unwrap();
            symlink(&old, &root).unwrap();
            assert_eq!(
                LinuxWorktree::reopen(&fx.policy, &binding).map(|_| ()),
                changed.clone(),
                "symlink to the original"
            );
            fs::remove_file(&root).unwrap();
            assert_eq!(
                LinuxWorktree::reopen(&fx.policy, &binding).map(|_| ()),
                changed.clone(),
                "missing"
            );
            fs::rename(&old, &root).unwrap();
            assert!(
                LinuxWorktree::reopen(&fx.policy, &binding).is_ok(),
                "restored original inode"
            );
            let mut b = binding.clone();
            b.ino += 1;
            assert_eq!(LinuxWorktree::reopen(&fx.policy, &b).map(|_| ()), changed);
            let denied = Err(WorktreeError::DeniedLocation);
            for path in [
                fx.outside.join("pai-task-0000beef"),
                fx.base.join("not-a-task"),
                fx.base.join("pai-task-XYZ00000"),
                fx.base.join("x/../pai-task-0000beef"),
            ] {
                let mut b = binding.clone();
                b.path = path.to_str().unwrap().into();
                assert_eq!(
                    LinuxWorktree::reopen(&fx.policy, &b).map(|_| ()),
                    denied.clone(),
                    "{path:?}"
                );
            }
            let mut tightened = fx.policy.clone();
            tightened.deny_within.push(fx.base.clone());
            assert_eq!(
                LinuxWorktree::reopen(&tightened, &binding).map(|_| ()),
                denied
            );
        }

        #[test]
        fn task_worktree_revert_restores_base() {
            use crate::task_diff::{diff_working_set, FileDecision, ReviewState, UiReviewEvent};
            use crate::task_workspace::{PathPolicy, ProposedEdit, WorkingSet};
            let base: BTreeMap<String, Vec<u8>> = [
                ("src/calc.py", &b"def add(a, b):\n    return a - b\n"[..]),
                ("src/util.py", b"U = 1\n"),
                ("README.md", b"# r\n"),
            ]
            .iter()
            .map(|(p, b)| ((*p).to_owned(), b.to_vec()))
            .collect();
            let mut ws = WorkingSet::from_parts(base.clone(), base.clone()).unwrap();
            let policy = PathPolicy::new(
                base.keys().cloned().collect(),
                BTreeSet::new(),
                vec!["src/".into()],
            );
            for e in [
                ProposedEdit::Replace {
                    path: "src/calc.py".into(),
                    content: "def add(a, b):\n    return a + b\n".into(),
                },
                ProposedEdit::Delete {
                    path: "src/util.py".into(),
                },
                ProposedEdit::Create {
                    path: "src/new.py".into(),
                    content: "N = 1\n".into(),
                },
            ] {
                ws.apply_edit(&policy, &e).unwrap();
            }
            let mut review = ReviewState::default();
            for d in diff_working_set(&ws) {
                let ev = UiReviewEvent {
                    task_id: "0".repeat(32),
                    view_seq: 3,
                    path: d.path.clone(),
                    decision: FileDecision::Accepted,
                    displayed_base_sha256: d.base_sha256.clone(),
                    displayed_new_sha256: d.new_sha256.clone(),
                };
                review
                    .record(&ws, &ev, "synthetic-review-not-a-human")
                    .unwrap();
            }
            let plan = review.apply_plan(&ws).unwrap();
            let fx = fixture();
            let wt = LinuxWorktree::create(&fx.policy, &tid(0xabc)).unwrap();
            let root = PathBuf::from(&wt.binding().path);
            apply_changes(&wt, &plan_changes(&BTreeMap::new(), &plan.files)).unwrap();
            let expected: BTreeMap<String, Vec<u8>> = plan
                .files
                .iter()
                .filter_map(|(p, b)| b.clone().map(|b| (p.clone(), b)))
                .collect();
            assert_eq!(tree(&root), expected);
            let paths: Vec<String> = plan.post_image.keys().cloned().collect();
            assert_eq!(observe(&wt, &paths).unwrap(), plan.post_image);
            // Out-of-band change after apply: revert refuses, writes nothing.
            let applied_calc = fs::read(root.join("src/calc.py")).unwrap();
            fs::write(root.join("src/calc.py"), b"# edited by someone\n").unwrap();
            let snapshot = tree(&root);
            assert_eq!(
                apply_changes(&wt, &revert_plan(&plan.post_image, &base)),
                Err(WorktreeError::PreImageMismatch {
                    path: "src/calc.py".into()
                })
            );
            assert_eq!(tree(&root), snapshot);
            fs::write(root.join("src/calc.py"), applied_calc).unwrap();
            // Revert: back to exactly the captured base.
            let reverted = apply_changes(&wt, &revert_plan(&plan.post_image, &base)).unwrap();
            assert_eq!(reverted, vec!["src/calc.py", "src/new.py", "src/util.py"]);
            assert_eq!(tree(&root), base);
            assert!(outside_intact(&fx.outside));
        }
    }
}
