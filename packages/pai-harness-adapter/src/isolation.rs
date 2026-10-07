//! Stage4 synchronous replay containment. No durable jobs, shell or host grants.
//! Linux requires the actually probed no-new-privileges/drop-caps+bwrap backend.
//! Other platforms fail BEFORE generated code is spawned. /proc is not mounted.
//! Snapshot admission is fd-relative (openat + O_NOFOLLOW + fstat) on Linux only;
//! non-Linux snapshot policies/captures fail closed with IsolationUnavailable.

use crate::knowledge_retrieval::TrustedSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub mod workspace;

pub const ISOLATION_PROFILE: &str = "linux-dropcaps-bwrap-no-proc-rofs-noipc-nouserns-prlimit-v2";
/// The ONLY writable filesystem inside containment (/tmp tmpfs). /, /dev (incl.
/// /dev/shm), /work and the runtime binds are read-only mounts.
pub const WRITABLE_TMPFS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILES: usize = 16;
const MAX_SNAPSHOT: usize = 1024 * 1024;
const PYTHON: &str = "/usr/bin/python3";
const SETPRIV: &str = "/usr/bin/setpriv";
const BWRAP: &str = "/usr/bin/bwrap";
const PRLIMIT: &str = "/usr/bin/prlimit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsolationError {
    IsolationUnavailable,
    DeniedRoot,
    InvalidInput,
    HashMismatch,
    Io,
}
impl std::fmt::Display for IsolationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "isolation: {self:?}")
    }
}
impl std::error::Error for IsolationError {}
type Result<T> = std::result::Result<T, IsolationError>;
fn io<T>(x: std::io::Result<T>) -> Result<T> {
    x.map_err(|_| IsolationError::Io)
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| IsolationError::InvalidInput)
}

/// Owner/controller-supplied allowlist, NOT model source attestation. Exact roots,
/// pinned to the directory inode (dev, ino) opened symlink-free at policy creation.
pub struct SnapshotPolicy {
    roots: Vec<(PathBuf, admission::DirId)>,
    scratch: PathBuf,
}
impl SnapshotPolicy {
    pub fn new(roots: Vec<PathBuf>, scratch: PathBuf) -> Result<Self> {
        if roots.is_empty() || roots.len() > 8 {
            return Err(IsolationError::DeniedRoot);
        }
        let scratch = io(fs::canonicalize(scratch))?;
        let mut checked = Vec::new();
        for root in roots {
            let root = io(fs::canonicalize(root))?;
            // Never admit a broad filesystem/user/runtime root as a project.
            if root.components().count() < 4
                || ["/usr", "/lib", "/lib64", "/etc", "/proc", "/dev", "/sys"]
                    .iter()
                    .any(|p| root.starts_with(p))
                || scratch.starts_with(&root)
            {
                return Err(IsolationError::DeniedRoot);
            }
            // Fails closed with IsolationUnavailable where no fd-relative
            // equivalent is implemented (every non-Linux target).
            let (_, id) = admission::open_dir(&root)?;
            checked.push((root, id));
        }
        Ok(Self {
            roots: checked,
            scratch,
        })
    }
    /// Explicit bounded file list; never scans or writes the source tree. Every
    /// file is read through an fd reached by openat(O_NOFOLLOW) beneath the pinned
    /// root fd: regular, single-link, same-device, bounded and fstat-stable.
    pub fn capture(
        &self,
        root: &Path,
        source: TrustedSource,
        primary: &str,
        files: &[String],
    ) -> Result<ProjectSnapshot> {
        let root = io(fs::canonicalize(root))?;
        let root_id = self
            .roots
            .iter()
            .find(|(r, _)| *r == root)
            .map(|(_, id)| *id)
            .ok_or(IsolationError::DeniedRoot)?;
        if files.is_empty()
            || files.len() > MAX_FILES
            || !files.iter().any(|p| p == primary)
            || source.source_id.is_empty()
            || source.source_id.len() > 512
            || source.source_version.is_empty()
            || source.source_version.len() > 256
            || ![40, 64].contains(&source.source_commit.len())
            || !source
                .source_commit
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(IsolationError::InvalidInput);
        }
        let (root_fd, opened) = admission::open_dir(&root)?;
        if opened != root_id {
            return Err(IsolationError::DeniedRoot);
        }
        let mut contents = BTreeMap::new();
        let mut total = 0;
        for path in files {
            relative(path)?;
            let bytes = admission::read_beneath(&root_fd, root_id, path)?;
            total += bytes.len();
            if total > MAX_SNAPSHOT || contents.insert(path.clone(), bytes).is_some() {
                return Err(IsolationError::InvalidInput);
            }
        }
        if hash(&contents[primary]) != source.file_digest {
            return Err(IsolationError::HashMismatch);
        }
        let dir = self.scratch.join(format!("pai-replay-{}", nonce()?));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            io(fs::DirBuilder::new().mode(0o700).create(&dir))?;
        }
        #[cfg(not(unix))]
        {
            io(fs::create_dir(&dir))?;
        }
        let dir_id = match admission::open_dir(&dir) {
            Ok((_, id)) => id,
            Err(e) => {
                let _ = fs::remove_dir(&dir);
                return Err(e);
            }
        };
        let mut snapshot = ProjectSnapshot {
            root,
            root_id,
            dir,
            dir_id,
            binding: SnapshotBinding {
                source,
                platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                primary: primary.into(),
                files: BTreeMap::new(),
                snapshot_sha256: String::new(),
            },
        };
        for (path, bytes) in contents {
            let target = snapshot.dir.join(&path);
            io(fs::create_dir_all(
                target.parent().ok_or(IsolationError::InvalidInput)?,
            ))?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            io(io(options.open(&target))?.write_all(&bytes))?;
            snapshot.binding.files.insert(path, hash(&bytes));
        }
        snapshot.binding.snapshot_sha256 = hash(&json(&snapshot.binding)?);
        readonly(&snapshot.dir)?;
        snapshot.validate()?;
        Ok(snapshot)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotBinding {
    pub source: TrustedSource,
    pub platform: String,
    pub primary: String,
    pub files: BTreeMap<String, String>,
    pub snapshot_sha256: String,
}
/// Private staging path, no public constructor/deserialize. Owner files stay untouched.
pub struct ProjectSnapshot {
    root: PathBuf,
    root_id: admission::DirId,
    dir: PathBuf,
    dir_id: admission::DirId,
    binding: SnapshotBinding,
}
impl ProjectSnapshot {
    pub fn binding(&self) -> &SnapshotBinding {
        &self.binding
    }
    pub fn validate(&self) -> Result<()> {
        let (dir, id) = admission::open_dir(&self.dir)?;
        if id != self.dir_id {
            return Err(IsolationError::HashMismatch);
        }
        for (path, digest) in &self.binding.files {
            if hash(&admission::read_beneath(&dir, id, path)?) != *digest {
                return Err(IsolationError::HashMismatch);
            }
        }
        let mut binding = self.binding.clone();
        binding.snapshot_sha256.clear();
        if hash(&json(&binding)?) != self.binding.snapshot_sha256 {
            return Err(IsolationError::HashMismatch);
        }
        Ok(())
    }
    /// Fresh identity check against the owner project, in addition to frozen bytes.
    /// Same fd-relative admission as capture, beneath the SAME pinned root inode.
    pub fn recheck_source(&self) -> Result<()> {
        self.validate()?;
        let (root, id) = admission::open_dir(&self.root)?;
        if id != self.root_id {
            return Err(IsolationError::DeniedRoot);
        }
        for (path, digest) in &self.binding.files {
            if hash(&admission::read_beneath(&root, id, path)?) != *digest {
                return Err(IsolationError::HashMismatch);
            }
        }
        Ok(())
    }
}
impl Drop for ProjectSnapshot {
    fn drop(&mut self) {
        let _ = writable(&self.dir);
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn relative(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 256
        || path.contains('\\')
        // Canonical names only: Path::components() silently collapses "a//b",
        // which would let two distinct selection names alias one file.
        || path
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
        || !Path::new(path)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
        || path.as_bytes().contains(&0)
    {
        return Err(IsolationError::InvalidInput);
    }
    Ok(())
}
/// B2 repair: bind every check to the inode actually opened. No path is checked
/// and then re-resolved; symlinks are never followed at any component.
#[cfg(target_os = "linux")]
mod admission {
    use super::{relative, IsolationError, Result};
    use rustix::fs::{fstat, openat, FileType, Mode, OFlags, Stat, CWD};
    use rustix::io::Errno;
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::{AsFd, OwnedFd};
    use std::path::{Component, Path};

    /// Per selected file byte bound (the snapshot total is bounded separately).
    const MAX_FILE: usize = 256 * 1024;
    pub(super) type DirFd = OwnedFd;
    /// Pinned directory identity (st_dev, st_ino) of an approved root or staging dir.
    #[derive(Debug, Clone, Copy)]
    pub(super) struct DirId(Stat);
    impl PartialEq for DirId {
        fn eq(&self, other: &Self) -> bool {
            self.0.st_dev == other.0.st_dev && self.0.st_ino == other.0.st_ino
        }
    }
    impl Eq for DirId {}
    /// openat(dirfd, name) that NEVER follows a symlink (O_NOFOLLOW) and never
    /// leaks the fd into children (O_CLOEXEC). `name` is a single component.
    fn open_nofollow<Fd: AsFd>(dir: Fd, name: &OsStr, flags: OFlags) -> Result<OwnedFd> {
        openat(
            dir,
            name,
            flags | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NOCTTY,
            Mode::empty(),
        )
        .map_err(|e| {
            // ELOOP/ENOTDIR: a symlink (or non-directory) where a real directory
            // or file must be. Always a denial, never treated as "missing".
            if e == Errno::LOOP || e == Errno::NOTDIR {
                IsolationError::DeniedRoot
            } else {
                IsolationError::Io
            }
        })
    }
    fn stat<Fd: AsFd>(fd: Fd) -> Result<Stat> {
        fstat(fd).map_err(|_| IsolationError::Io)
    }
    fn kind(st: &Stat) -> FileType {
        FileType::from_raw_mode(st.st_mode)
    }
    /// Open an absolute canonical directory from "/" one component at a time with
    /// O_DIRECTORY|O_NOFOLLOW, so no ancestor symlink is ever followed.
    pub(super) fn open_dir(path: &Path) -> Result<(DirFd, DirId)> {
        let mut parts = path.components();
        if parts.next() != Some(Component::RootDir) {
            return Err(IsolationError::DeniedRoot);
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY;
        let mut fd = open_nofollow(CWD, OsStr::new("/"), flags)?;
        for part in parts {
            match part {
                Component::Normal(name) => fd = open_nofollow(&fd, name, flags)?,
                _ => return Err(IsolationError::DeniedRoot),
            }
        }
        let st = stat(&fd)?;
        if kind(&st) != FileType::Directory {
            return Err(IsolationError::DeniedRoot);
        }
        Ok((fd, DirId(st)))
    }
    fn stable(a: &Stat, b: &Stat) -> bool {
        a.st_dev == b.st_dev
            && a.st_ino == b.st_ino
            && a.st_mode == b.st_mode
            && a.st_nlink == b.st_nlink
            && a.st_size == b.st_size
            && a.st_mtime == b.st_mtime
            && a.st_mtime_nsec == b.st_mtime_nsec
            && a.st_ctime == b.st_ctime
            && a.st_ctime_nsec == b.st_ctime_nsec
    }
    /// Read `path` strictly beneath `root` (identity `id`): intermediates opened
    /// O_DIRECTORY|O_NOFOLLOW on the root device; the final fd must be a regular
    /// file with exactly one link (hard links rejected outright), on the root
    /// device, within MAX_FILE, read through that same fd, with unchanged
    /// dev/ino/mode/nlink/size/mtime/ctime after the read.
    pub(super) fn read_beneath(root: &DirFd, id: DirId, path: &str) -> Result<Vec<u8>> {
        relative(path)?;
        let names = Path::new(path)
            .components()
            .map(|c| match c {
                Component::Normal(name) => Ok(name),
                _ => Err(IsolationError::InvalidInput),
            })
            .collect::<Result<Vec<_>>>()?;
        let (last, dirs) = names.split_last().ok_or(IsolationError::InvalidInput)?;
        let mut current: Option<OwnedFd> = None;
        for name in dirs {
            let parent = current.as_ref().unwrap_or(root);
            let next = open_nofollow(parent, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
            let st = stat(&next)?;
            if kind(&st) != FileType::Directory || st.st_dev != id.0.st_dev {
                return Err(IsolationError::DeniedRoot);
            }
            current = Some(next);
        }
        let parent = current.as_ref().unwrap_or(root);
        // O_NONBLOCK: a FIFO/device can never block admission; it is then refused.
        let fd = open_nofollow(parent, last, OFlags::RDONLY | OFlags::NONBLOCK)?;
        let before = stat(&fd)?;
        if kind(&before) != FileType::RegularFile {
            return Err(IsolationError::InvalidInput);
        }
        if before.st_nlink != 1 || before.st_dev != id.0.st_dev {
            return Err(IsolationError::DeniedRoot);
        }
        let size = u64::try_from(before.st_size).map_err(|_| IsolationError::InvalidInput)?;
        if size > MAX_FILE as u64 {
            return Err(IsolationError::InvalidInput);
        }
        let mut file = File::from(fd);
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_FILE as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| IsolationError::Io)?;
        let after = stat(&file)?;
        if bytes.len() as u64 != size || !stable(&before, &after) {
            return Err(IsolationError::HashMismatch);
        }
        Ok(bytes)
    }
}
/// No correct fd-relative equivalent is implemented off Linux: fail closed. This
/// is NOT a Windows/macOS safety claim; nothing is captured or spawned there.
#[cfg(not(target_os = "linux"))]
mod admission {
    use super::{IsolationError, Result};
    use std::path::Path;
    pub(super) struct DirFd;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct DirId;
    pub(super) fn open_dir(_: &Path) -> Result<(DirFd, DirId)> {
        Err(IsolationError::IsolationUnavailable)
    }
    pub(super) fn read_beneath(_: &DirFd, _: DirId, _: &str) -> Result<Vec<u8>> {
        Err(IsolationError::IsolationUnavailable)
    }
}
fn readonly(path: &Path) -> Result<()> {
    permissions(path, true)
}
fn writable(path: &Path) -> Result<()> {
    permissions(path, false)
}
fn permissions(path: &Path, read_only: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.is_dir() {
            if !read_only {
                io(fs::set_permissions(path, fs::Permissions::from_mode(0o700)))?;
            }
            for p in io(fs::read_dir(path))? {
                permissions(&io(p)?.path(), read_only)?;
            }
        }
        let mode = if path.is_dir() {
            if read_only {
                0o500
            } else {
                0o700
            }
        } else if read_only {
            0o400
        } else {
            0o600
        };
        io(fs::set_permissions(path, fs::Permissions::from_mode(mode)))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, read_only);
    }
    Ok(())
}
pub(crate) fn nonce() -> Result<String> {
    let mut bytes = [0u8; 16];
    io(io(File::open("/dev/urandom"))?.read_exact(&mut bytes))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunLimits {
    pub cpu_seconds: u32,
    pub memory_bytes: u64,
    pub processes: u32,
    pub timeout_ms: u64,
    pub output_bytes: usize,
}
impl RunLimits {
    pub fn validate(&self) -> Result<()> {
        if !(1..=5).contains(&self.cpu_seconds)
            || !(64 * 1024 * 1024..=256 * 1024 * 1024).contains(&self.memory_bytes)
            || !(8..=32).contains(&self.processes)
            || !(50..=5000).contains(&self.timeout_ms)
            || !(64..=4096).contains(&self.output_bytes)
        {
            return Err(IsolationError::InvalidInput);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default)]
pub struct Cancellation {
    flag: Arc<AtomicBool>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Termination {
    Completed,
    Timeout,
    OutputLimit,
    Cancelled,
    RunnerFailure,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActualRun {
    pub argv: Vec<String>,
    pub status: Option<i32>,
    pub termination: Termination,
    pub stdout_hex: String,
    pub stderr_hex: String,
    pub log_sha256: String,
    pub output_files: BTreeMap<String, String>,
    pub elapsed_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProfile {
    pub schema: String,
    pub backend: String,
    pub platform: String,
    pub program_hashes: BTreeMap<String, String>,
    pub supervisor_sha256: String,
    pub proc_mounted: bool,
}
/// Installed program identities captured by the trusted controller, not caller JSON.
pub struct LinuxIsolation {
    profile: RuntimeProfile,
}
impl LinuxIsolation {
    pub fn new() -> Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err(IsolationError::IsolationUnavailable);
        }
        let mut hashes = BTreeMap::new();
        for p in [SETPRIV, BWRAP, PRLIMIT, PYTHON] {
            hashes.insert(p.into(), program_hash(p)?);
        }
        let backend = Self {
            profile: RuntimeProfile {
                schema: "pai.runtime-profile.v1".into(),
                backend: ISOLATION_PROFILE.into(),
                platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                program_hashes: hashes,
                supervisor_sha256: hash(SUPERVISOR.as_bytes()),
                proc_mounted: false,
            },
        };
        backend.preflight()?;
        // Real non-generated readiness probe under exactly the same wrapper.
        let result=backend.command(None,&RunLimits {cpu_seconds:1,memory_bytes:64*1024*1024,processes:32,timeout_ms:1000,output_bytes:1024})
            .args(["-I","-c", "import os,socket,ctypes; assert not os.path.exists('/proc'); assert not os.path.exists('/agent'); assert not os.path.exists('/home'); assert os.getuid()!=0; assert set(os.environ)<= {'PATH','HOME','PWD','LC_CTYPE'}; assert all(os.statvfs(d).f_flag&1 for d in ('/','/dev','/dev/shm','/work','/usr')); t=os.statvfs('/tmp'); assert not t.f_flag&1 and t.f_blocks*t.f_frsize==16777216; assert ctypes.CDLL(None,use_errno=True).unshare(0x10000000)!=0; print('READY')"])
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_|IsolationError::IsolationUnavailable)?;
        let probe = bounded_wait(
            result,
            Duration::from_millis(1500),
            &Cancellation::default(),
        )?;
        if probe.0 != Termination::Completed || probe.1 != Some(0) || probe.2 != b"READY\n" {
            return Err(IsolationError::IsolationUnavailable);
        }
        Ok(backend)
    }
    pub fn profile(&self) -> &RuntimeProfile {
        &self.profile
    }
    fn preflight(&self) -> Result<()> {
        if !cfg!(target_os = "linux") || self.profile.program_hashes.len() != 4 {
            return Err(IsolationError::IsolationUnavailable);
        }
        for p in [SETPRIV, BWRAP, PRLIMIT, PYTHON] {
            if self.profile.program_hashes.get(p) != Some(&program_hash(p)?) {
                return Err(IsolationError::IsolationUnavailable);
            }
        }
        Ok(())
    }
    fn command(&self, snapshot: Option<&ProjectSnapshot>, limits: &RunLimits) -> Command {
        let mut c = Command::new(SETPRIV);
        c.env_clear().stdin(Stdio::null());
        c.args([
            "--no-new-privs",
            "--inh-caps=-all",
            "--ambient-caps=-all",
            "--bounding-set=-all",
            BWRAP,
            "--unshare-all",
            // Explicit user namespace so bwrap can cap nested user namespaces at
            // zero (unshare(CLONE_NEWUSER) -> ENOSPC inside the sandbox).
            "--unshare-user",
            "--disable-userns",
            "--die-with-parent",
            "--new-session",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind",
            "/lib",
            "/lib",
            "--ro-bind",
            "/lib64",
            "/lib64",
            // bwrap's --dev is an unsized tmpfs (incl. /dev/shm). Remount it
            // read-only (non-recursive): device-node binds such as /dev/null stay
            // usable, but nothing can be created under /dev or /dev/shm.
            "--dev",
            "/dev",
            "--remount-ro",
            "/dev",
        ]);
        // --size applies to the NEXT tmpfs only: the single writable filesystem.
        c.arg("--size")
            .arg(WRITABLE_TMPFS_BYTES.to_string())
            .args(["--tmpfs", "/tmp"]);
        if let Some(s) = snapshot {
            c.arg("--ro-bind").arg(&s.dir).arg("/work");
        } else {
            c.args(["--dir", "/work"]);
        }
        c.args([
            // The sandbox root itself is an unsized tmpfs: make it read-only last,
            // after every mount point above has been created.
            "--remount-ro",
            "/",
            "--chdir",
            "/work",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin",
            "--setenv",
            "HOME",
            "/tmp",
            "--",
            PRLIMIT,
        ]);
        c.arg(format!("--cpu={0}:{0}", limits.cpu_seconds))
            .arg(format!("--as={0}:{0}", limits.memory_bytes))
            .arg(format!("--nproc={0}:{0}", limits.processes))
            .args([
                "--fsize=4096:4096",
                "--nofile=32:32",
                "--core=0:0",
                "--",
                PYTHON,
            ]);
        c
    }
    pub(crate) fn run(
        &self,
        snapshot: &ProjectSnapshot,
        argv: &[String],
        outputs: &[String],
        limits: &RunLimits,
        cancel: &Cancellation,
    ) -> Result<ActualRun> {
        self.preflight()?;
        limits.validate()?;
        snapshot.validate()?;
        if argv.len() < 3
            || argv.len() > 16
            || argv[0] != PYTHON
            || argv[1] != "-I"
            || !argv[2].starts_with("/work/")
            || !snapshot.binding.files.contains_key(&argv[2][6..])
            || argv.iter().any(|s| s.len() > 256 || s.contains('\0'))
            || outputs.len() > 4
            || outputs.iter().any(|s| {
                !s.starts_with("/tmp/") || relative(&s[5..]).is_err() || s[5..].contains('/')
            })
        {
            return Err(IsolationError::InvalidInput);
        }
        let start = Instant::now();
        let mut run = ActualRun {
            argv: argv.to_vec(),
            status: None,
            termination: Termination::Cancelled,
            stdout_hex: String::new(),
            stderr_hex: String::new(),
            log_sha256: String::new(),
            output_files: BTreeMap::new(),
            elapsed_ms: 0,
        };
        if !cancel.is_cancelled() {
            let request = serde_json::json!({"argv":argv,"outputs":outputs,"timeout_ms":limits.timeout_ms,"output_bytes":limits.output_bytes,"program_sha256":self.profile.program_hashes[PYTHON]});
            let child = self
                .command(Some(snapshot), limits)
                .args(["-I", "-c", SUPERVISOR, &request.to_string()])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|_| IsolationError::IsolationUnavailable)?;
            let (termination, status, out, err) = bounded_wait(
                child,
                Duration::from_millis(limits.timeout_ms + 500),
                cancel,
            )?;
            if termination == Termination::Completed && status == Some(0) {
                let raw: RawRun = serde_json::from_slice(&out).map_err(|_| IsolationError::Io)?;
                if raw.stdout_hex.len() > limits.output_bytes * 2
                    || raw.stderr_hex.len() > limits.output_bytes * 2
                    || raw.output_files.keys().any(|p| !outputs.contains(p))
                    || raw
                        .output_files
                        .values()
                        .any(|h| !unoone_capability_contracts::knowledge::valid_digest(h))
                    || !err.is_empty()
                {
                    return Err(IsolationError::Io);
                }
                run.status = raw.status;
                run.termination = raw.termination;
                run.stdout_hex = raw.stdout_hex;
                run.stderr_hex = raw.stderr_hex;
                run.output_files = raw.output_files;
            } else {
                run.termination = if termination == Termination::Completed {
                    Termination::RunnerFailure
                } else {
                    termination
                };
                run.stderr_hex = hex(&err);
            }
        }
        snapshot.validate()?;
        run.elapsed_ms = start.elapsed().as_millis() as u64;
        run.log_sha256 = log_hash(&run)?;
        Ok(run)
    }
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn log_hash(run: &ActualRun) -> Result<String> {
    Ok(hash(&json(&(
        run.status,
        run.termination,
        &run.stdout_hex,
        &run.stderr_hex,
        &run.output_files,
    ))?))
}
fn program_hash(path: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let f = File::open(path).map_err(|_| IsolationError::IsolationUnavailable)?;
    f.take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| IsolationError::IsolationUnavailable)?;
    if bytes.is_empty() || bytes.len() > 32 * 1024 * 1024 {
        return Err(IsolationError::IsolationUnavailable);
    }
    Ok(hash(&bytes))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRun {
    status: Option<i32>,
    termination: Termination,
    stdout_hex: String,
    stderr_hex: String,
    output_files: BTreeMap<String, String>,
}
/// Receipt-channel cap of every Stage 4 run (value unchanged by Stage 5 X2).
const RECEIPT_CAP: usize = 32 * 1024;
fn drain<R: Read + Send + 'static>(
    mut pipe: R,
    over: Arc<AtomicBool>,
    cap: usize,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            let n = pipe.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if bytes.len() + n > cap {
                over.store(true, Ordering::SeqCst);
            } else {
                bytes.extend_from_slice(&buf[..n]);
            }
        }
        Ok(bytes)
    })
}
type WaitResult = (Termination, Option<i32>, Vec<u8>, Vec<u8>);
fn bounded_wait(
    child: std::process::Child,
    timeout: Duration,
    cancel: &Cancellation,
) -> Result<WaitResult> {
    bounded_wait_capped(child, timeout, cancel, RECEIPT_CAP)
}
/// Stage 5 X2: identical kill/timeout/cancel logic with a caller-computed
/// receipt-channel cap. `bounded_wait` passes RECEIPT_CAP (Stage 4 unchanged).
fn bounded_wait_capped(
    mut child: std::process::Child,
    timeout: Duration,
    cancel: &Cancellation,
    cap: usize,
) -> Result<WaitResult> {
    let over = Arc::new(AtomicBool::new(false));
    let out = drain(
        child.stdout.take().ok_or(IsolationError::Io)?,
        over.clone(),
        cap,
    );
    let err = drain(
        child.stderr.take().ok_or(IsolationError::Io)?,
        over.clone(),
        cap,
    );
    let start = Instant::now();
    let mut termination = Termination::Completed;
    let status = loop {
        if cancel.is_cancelled() {
            termination = Termination::Cancelled;
        } else if over.load(Ordering::SeqCst) {
            termination = Termination::OutputLimit;
        } else if start.elapsed() >= timeout {
            termination = Termination::Timeout;
        }
        if termination != Termination::Completed {
            let _ = child.kill();
            break io(child.wait())?;
        }
        if let Some(status) = io(child.try_wait())? {
            break status;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let stdout = io(out.join().map_err(|_| IsolationError::Io)?)?;
    let stderr = io(err.join().map_err(|_| IsolationError::Io)?)?;
    Ok((termination, status.code(), stdout, stderr))
}
// Trusted supervisor runs INSIDE containment, child pipes are never the receipt
// channel. Independent file hashing uses O_NOFOLLOW + regular-file/size checks.
// Same-UID interference can terminate the supervisor, but cannot mint a host MAC.
const SUPERVISOR: &str = r#"import os,sys,json,subprocess,threading,time,hashlib,stat,ctypes,platform
# Drop-capabilities alone does not deny same-UID ptrace/process_vm_writev on
# kernels without Yama. Protect the supervisor before ANY generated child.
libc=ctypes.CDLL(None,use_errno=True)
assert libc.prctl(4,0,0,0,0)==0 and libc.prctl(3,0,0,0,0)==0
# Inherited, irremovable seccomp filter (no_new_privs is already set): SysV
# shmget/semget/msgget create memory outside RLIMIT_AS -> EPERM; any non-x86_64
# audit arch or x32 syscall -> EPERM. Everything else is allowed (not a sandbox
# by itself). Unsupported machines fail closed here, before any child exists.
assert platform.machine()=='x86_64'
class SF(ctypes.Structure):_fields_=[('code',ctypes.c_ushort),('jt',ctypes.c_ubyte),('jf',ctypes.c_ubyte),('k',ctypes.c_uint32)]
class SP(ctypes.Structure):_fields_=[('len',ctypes.c_ushort),('filter',ctypes.POINTER(SF))]
prog=(SF*9)(SF(0x20,0,0,4),SF(0x15,0,6,0xc000003e),SF(0x20,0,0,0),SF(0x35,4,0,0x40000000),SF(0x15,3,0,29),SF(0x15,2,0,64),SF(0x15,1,0,68),SF(0x06,0,0,0x7fff0000),SF(0x06,0,0,0x00050001))
fprog=SP(9,ctypes.cast(prog,ctypes.POINTER(SF)))
assert libc.prctl(ctypes.c_int(22),ctypes.c_ulong(2),ctypes.byref(fprog),ctypes.c_ulong(0),ctypes.c_ulong(0))==0
r=json.loads(sys.argv[1])
assert hashlib.sha256(open('/usr/bin/python3','rb').read()).hexdigest()==r['program_sha256']
p=subprocess.Popen(r['argv'],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,close_fds=True,env={'PATH':'/usr/bin','HOME':'/tmp'})
limit=r['output_bytes']; buffers=[bytearray(),bytearray()]; over=threading.Event()
def drain(pipe,i):
 while True:
  b=pipe.read(1024)
  if not b: break
  if len(buffers[i])+len(b)>limit: over.set()
  else: buffers[i].extend(b)
threads=[threading.Thread(target=drain,args=(p.stdout,0)),threading.Thread(target=drain,args=(p.stderr,1))]
for t in threads:t.start()
start=time.monotonic(); termination='completed'
while p.poll() is None:
 if over.is_set():termination='output_limit';p.kill();break
 if (time.monotonic()-start)*1000>=r['timeout_ms']:termination='timeout';p.kill();break
 time.sleep(.005)
status=p.wait()
for t in threads:t.join()
if over.is_set():termination='output_limit'
files={}
for path in r['outputs']:
 try:
  fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
  with os.fdopen(fd,'rb') as f:
   s=os.fstat(f.fileno())
   if stat.S_ISREG(s.st_mode) and s.st_size<=4096:
    b=f.read(4097)
    if len(b)<=4096:files[path]=hashlib.sha256(b).hexdigest()
 except OSError:pass
print(json.dumps({'status':status,'termination':termination,'stdout_hex':buffers[0].hex(),'stderr_hex':buffers[1].hex(),'output_files':files},separators=(',',':')))
"#;

/// Test-only precondition for sandbox-dependent tests.
///
/// Real Linux isolation is a precondition: without it these tests FAIL (they
/// never silently pass). The single exception is a CI runner (`CI=true`, which
/// GitHub Actions sets) whose image lacks the isolation prerequisites
/// (setpriv, bubblewrap, prlimit, unprivileged user namespaces): there the test
/// is skipped and a visible `::warning::` annotation is written to the real
/// stderr (bypassing libtest capture). `UNOONE_REQUIRE_ISOLATION=1` forbids
/// skipping even on CI; the project's verification gates set it.
#[cfg(test)]
pub(crate) mod test_support {
    use std::io::Write;
    use std::sync::OnceLock;

    pub(crate) fn isolation_or_ci_skip(test: &str) -> bool {
        static AVAILABLE: OnceLock<bool> = OnceLock::new();
        // A few attempts: the readiness probe has a short deadline and the
        // suite may be running many sandbox tests in parallel.
        if *AVAILABLE.get_or_init(|| (0..3).any(|_| super::LinuxIsolation::new().is_ok())) {
            return true;
        }
        let flag = |name: &str| std::env::var(name).is_ok_and(|v| v == "1" || v == "true");
        if !flag("CI") || flag("UNOONE_REQUIRE_ISOLATION") {
            panic!(
                "{test}: real Linux isolation is a test precondition (needs /usr/bin/setpriv, \
                 /usr/bin/bwrap, /usr/bin/prlimit, /usr/bin/python3 and unprivileged user \
                 namespaces): IsolationUnavailable"
            );
        }
        static WARNED: OnceLock<()> = OnceLock::new();
        let mut err = std::io::stderr();
        WARNED.get_or_init(|| {
            let _ = writeln!(
                err,
                "::warning title=UnoOne sandbox tests skipped::Linux isolation (setpriv, \
                 bubblewrap, prlimit, unprivileged user namespaces) is unavailable on this CI \
                 runner; sandbox-dependent tests were skipped, not passed."
            );
        });
        let _ = writeln!(
            err,
            "SKIPPED {test}: Linux isolation unavailable on this CI runner"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_names_must_be_canonical() {
        for good in ["calc.py", "inner/x.py", "a/b/c.txt"] {
            assert!(relative(good).is_ok(), "{good}");
        }
        for bad in [
            "", "/abs", "../x", "./x", "a/../b", "a//b", "a/./b", "a/", "/", "a\\b", "a\0b",
        ] {
            assert!(relative(bad).is_err(), "{bad:?}");
        }
    }
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "needs the Linux isolation backend (non-Linux fails closed before any spawn)"
    )]
    fn isolation_actual_linux_bounds_and_snapshot_rejections() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "isolation_actual_linux_bounds_and_snapshot_rejections",
        ) {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        let code=b"import os,socket,resource\nassert not os.path.exists('/proc') and not os.path.exists('/agent')\nassert os.getuid()!=0 and resource.getrlimit(resource.RLIMIT_AS)[0]<=134217728\ntry: open('/work/main.py','w'); raise Exception('writable')\nexcept OSError: pass\nassert socket.gethostname()!=os.environ.get('HOSTNAME','host')\nassert not os.environ.get('API_KEY')\nopen('/tmp/result','w').write('correct')\nprint('isolated')\n";
        fs::write(root.join("main.py"), code).unwrap();
        assert!(matches!(
            SnapshotPolicy::new(vec![PathBuf::from("/")], temp.path().to_owned()),
            Err(IsolationError::DeniedRoot)
        ));
        let policy = SnapshotPolicy::new(vec![root.clone()], temp.path().to_owned()).unwrap();
        let source = TrustedSource {
            source_id: "synthetic".into(),
            source_version: "1".into(),
            source_commit: "a".repeat(40),
            file_digest: hash(code),
        };
        assert!(matches!(
            policy.capture(temp.path(), source.clone(), "main.py", &["main.py".into()]),
            Err(IsolationError::DeniedRoot)
        ));
        assert!(policy
            .capture(&root, source.clone(), "../main.py", &["../main.py".into()])
            .is_err());
        let mut bad = source.clone();
        bad.file_digest = "0".repeat(64);
        assert!(policy
            .capture(&root, bad, "main.py", &["main.py".into()])
            .is_err());
        let snapshot = policy
            .capture(&root, source.clone(), "main.py", &["main.py".into()])
            .unwrap();
        let backend =
            LinuxIsolation::new().expect("real isolation is a test precondition, never skip");
        let limits = RunLimits {
            cpu_seconds: 2,
            memory_bytes: 128 * 1024 * 1024,
            processes: 32,
            timeout_ms: 1000,
            output_bytes: 1024,
        };
        let argv = vec![PYTHON.into(), "-I".into(), "/work/main.py".into()];
        let actual = backend
            .run(
                &snapshot,
                &argv,
                &["/tmp/result".into()],
                &limits,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(actual.status, Some(0));
        assert_eq!(actual.stdout_hex, hex(b"isolated\n"));
        assert_eq!(actual.output_files["/tmp/result"], hash(b"correct"));
        let cancel = Cancellation::default();
        cancel.cancel();
        assert_eq!(
            backend
                .run(&snapshot, &argv, &[], &limits, &cancel)
                .unwrap()
                .termination,
            Termination::Cancelled
        );
        fs::write(root.join("main.py"), b"import time; time.sleep(2)").unwrap();
        assert!(snapshot.recheck_source().is_err());
        let mut source = source;
        source.file_digest = hash(b"import time; time.sleep(2)");
        let slow = policy
            .capture(&root, source.clone(), "main.py", &["main.py".into()])
            .unwrap();
        let mut short = limits.clone();
        short.timeout_ms = 100;
        assert_eq!(
            backend
                .run(&slow, &argv, &[], &short, &Cancellation::default())
                .unwrap()
                .termination,
            Termination::Timeout
        );
        let cancel = Cancellation::default();
        let worker = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            worker.cancel();
        });
        assert_eq!(
            backend
                .run(&slow, &argv, &[], &limits, &cancel)
                .unwrap()
                .termination,
            Termination::Cancelled
        );
        fs::write(root.join("main.py"), b"print('x'*20000)").unwrap();
        source.file_digest = hash(b"print('x'*20000)");
        let loud = policy
            .capture(&root, source, "main.py", &["main.py".into()])
            .unwrap();
        assert_eq!(
            backend
                .run(&loud, &argv, &[], &limits, &Cancellation::default())
                .unwrap()
                .termination,
            Termination::OutputLimit
        );
        // Actual CPU kill, allocation failure, fork cap and independent symlink
        // postcondition refusal. These scripts run only inside this same backend.
        for (code,expected) in [
            ("import os,ctypes,errno\nl=ctypes.CDLL(None,use_errno=True); assert l.ptrace(16,os.getppid(),0,0)==-1 and ctypes.get_errno()==errno.EPERM; print('bounded')",Some("bounded\n")),
            ("import errno\ntry:\n for i in range(5000):\n  with open('/tmp/f'+str(i),'wb') as f: f.write(b'x'*4096)\nexcept OSError as e:\n assert e.errno==errno.ENOSPC; print('bounded')\nelse: raise Exception('tmpfs uncapped')",Some("bounded\n")),
            ("while True: pass",None),
            ("try: x=bytearray(200*1024*1024)\nexcept MemoryError: print('bounded')",Some("bounded\n")),
            ("import os,errno\npids=[]\ntry:\n for i in range(64):\n  p=os.fork()\n  if p==0: os._exit(0)\n  pids.append(p)\nexcept OSError as e:\n assert e.errno==errno.EAGAIN; print('bounded')\nelse: raise Exception('uncapped')\nfor p in pids: os.waitpid(p,0)",Some("bounded\n")),
            ("import os; os.symlink('/work/main.py','/tmp/result'); print('bounded')",Some("bounded\n"))
        ] {
            fs::write(root.join("main.py"),code).unwrap();let s=TrustedSource {source_id:"synthetic".into(),source_version:"1".into(),source_commit:"a".repeat(40),file_digest:hash(code.as_bytes())};
            let snap=policy.capture(&root,s,"main.py",&["main.py".into()]).unwrap();let mut bound=limits.clone();bound.cpu_seconds=1;bound.timeout_ms=3000;
            let result=backend.run(&snap,&argv,&["/tmp/result".into()],&bound,&Cancellation::default()).unwrap();
            if let Some(stdout)=expected {assert_eq!(result.status,Some(0));assert_eq!(result.stdout_hex,hex(stdout.as_bytes()));} else {assert_eq!(result.status,Some(-9));}
            assert!(result.output_files.is_empty());
        }
        println!(
            "STAGE4_ISOLATION_PROFILE {}",
            serde_json::to_string(backend.profile()).unwrap()
        );
        let proof_code="import sys,json; print(json.dumps({'status':0,'termination':'completed','verified':True})); sys.exit(7)";
        fs::write(root.join("main.py"), proof_code).unwrap();
        let proof_source = TrustedSource {
            source_id: "synthetic".into(),
            source_version: "1".into(),
            source_commit: "a".repeat(40),
            file_digest: hash(proof_code.as_bytes()),
        };
        let proof_snapshot = policy
            .capture(&root, proof_source, "main.py", &["main.py".into()])
            .unwrap();
        let proof_result = backend
            .run(
                &proof_snapshot,
                &argv,
                &[],
                &limits,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(proof_result.status, Some(7));
        assert_eq!(proof_result.termination, Termination::Completed);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let staged = snapshot.dir.join("main.py");
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&staged, b"print('fabricated')").unwrap();
            assert!(matches!(
                snapshot.validate(),
                Err(IsolationError::HashMismatch)
            ));
            fs::write(&staged, code).unwrap();
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o400)).unwrap();
        }
        let mut missing = backend;
        missing.profile.program_hashes.remove(BWRAP);
        assert!(matches!(
            missing.run(&snapshot, &argv, &[], &limits, &Cancellation::default()),
            Err(IsolationError::IsolationUnavailable)
        ));
    }

    // ---- Stage4 repair regressions (B2 snapshot admission, B4 writable bounds) ----
    #[cfg(target_os = "linux")]
    const ALLOWED: &[u8] = b"allowed-project-file";
    #[cfg(target_os = "linux")]
    const OUTSIDE: &[u8] = b"SYNTHETIC_OUTSIDE_ALLOWLIST_CONTENT";
    #[cfg(target_os = "linux")]
    fn race_fixture() -> (
        tempfile::TempDir,
        PathBuf,
        PathBuf,
        SnapshotPolicy,
        TrustedSource,
    ) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("project");
        let foreign = t.path().join("outside-approved-root");
        fs::create_dir_all(root.join("inner")).unwrap();
        fs::create_dir(&foreign).unwrap();
        let primary: &[u8] = b"# synthetic non-executed primary\n";
        fs::write(root.join("main.py"), primary).unwrap();
        fs::write(root.join("inner/payload.txt"), ALLOWED).unwrap();
        fs::write(foreign.join("payload.txt"), OUTSIDE).unwrap();
        let scratch = t.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        let policy = SnapshotPolicy::new(vec![root.clone()], scratch).unwrap();
        let source = TrustedSource {
            source_id: "race".into(),
            source_version: "1".into(),
            source_commit: "a".repeat(40),
            file_digest: hash(primary),
        };
        (t, root, foreign, policy, source)
    }
    #[cfg(target_os = "linux")]
    fn race_files() -> Vec<String> {
        vec!["main.py".into(), "inner/payload.txt".into()]
    }
    /// Concurrent rename-only swapper vs. capture. Escaping means a snapshot
    /// containing bytes from outside the approved root. Bounded, never a pass by
    /// timeout: the loop counts attempts and requires the swapper actually ran.
    #[cfg(target_os = "linux")]
    fn swap_race(kind: &str) {
        use std::os::unix::fs::symlink;
        let (_t, root, foreign, policy, source) = race_fixture();
        let files = race_files();
        let control = policy
            .capture(&root, source.clone(), "main.py", &files)
            .unwrap();
        assert_eq!(control.binding().files["inner/payload.txt"], hash(ALLOWED));
        drop(control);
        let (a, b, c) = if kind == "dir" {
            symlink(&foreign, root.join("inner-link")).unwrap();
            (
                root.join("inner"),
                root.join("inner-off"),
                root.join("inner-link"),
            )
        } else {
            symlink(foreign.join("payload.txt"), root.join("inner/payload.link")).unwrap();
            (
                root.join("inner/payload.txt"),
                root.join("inner/payload.off"),
                root.join("inner/payload.link"),
            )
        };
        let stop = Arc::new(AtomicBool::new(false));
        let swaps = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (flag, n) = (stop.clone(), swaps.clone());
        let worker = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                fs::rename(&a, &b).unwrap();
                fs::rename(&c, &a).unwrap();
                fs::rename(&a, &c).unwrap();
                fs::rename(&b, &a).unwrap();
                n.fetch_add(1, Ordering::Relaxed);
            }
        });
        let start = Instant::now();
        let (mut attempts, mut inside, mut errors, mut escaped) = (0u64, 0u64, 0u64, 0u64);
        while start.elapsed() < Duration::from_secs(5) && escaped == 0 {
            attempts += 1;
            match policy.capture(&root, source.clone(), "main.py", &files) {
                Ok(s) => {
                    if s.binding().files["inner/payload.txt"] == hash(OUTSIDE) {
                        escaped += 1;
                    } else {
                        inside += 1;
                    }
                }
                Err(_) => errors += 1,
            }
        }
        stop.store(true, Ordering::SeqCst);
        worker.join().unwrap();
        let swaps = swaps.load(Ordering::Relaxed);
        println!(
            "STAGE4_REPAIR_B2_RACE kind={kind} escaped={escaped} attempts={attempts} inside={inside} errors={errors} swaps={swaps} seconds={:.2}",
            start.elapsed().as_secs_f64()
        );
        assert!(swaps > 0 && attempts > 100, "race not exercised");
        assert_eq!(
            escaped, 0,
            "snapshot captured bytes from outside the approved root"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn repair_b2_directory_component_swap_race_fails_closed() {
        swap_race("dir");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn repair_b2_final_component_swap_race_fails_closed() {
        swap_race("final");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn repair_b2_hard_links_and_root_replacement_fail_closed() {
        let (t, root, foreign, policy, source) = race_fixture();
        let files = race_files();
        let good = policy
            .capture(&root, source.clone(), "main.py", &files)
            .unwrap();
        let mut results = Vec::new();
        // Static hard link to an outside inode with outside bytes (reviewer repro).
        fs::remove_file(root.join("inner/payload.txt")).unwrap();
        fs::hard_link(foreign.join("payload.txt"), root.join("inner/payload.txt")).unwrap();
        results.push((
            "capture_outside_hard_link",
            policy
                .capture(&root, source.clone(), "main.py", &files)
                .is_err(),
        ));
        // Same bytes behind an outside hard link: content-only rechecks cannot see it.
        let twin = t.path().join("outside-twin");
        fs::create_dir(&twin).unwrap();
        fs::write(twin.join("payload.txt"), ALLOWED).unwrap();
        fs::remove_file(root.join("inner/payload.txt")).unwrap();
        fs::hard_link(twin.join("payload.txt"), root.join("inner/payload.txt")).unwrap();
        results.push((
            "recheck_identical_outside_hard_link",
            good.recheck_source().is_err(),
        ));
        results.push((
            "capture_identical_outside_hard_link",
            policy
                .capture(&root, source.clone(), "main.py", &files)
                .is_err(),
        ));
        fs::remove_file(root.join("inner/payload.txt")).unwrap();
        fs::write(root.join("inner/payload.txt"), ALLOWED).unwrap();
        assert!(
            good.recheck_source().is_ok(),
            "control: restored regular file"
        );
        // Hard links are rejected outright, even when both names are inside the root.
        fs::hard_link(root.join("main.py"), root.join("inner/alias.py")).unwrap();
        results.push((
            "capture_in_root_hard_link",
            policy
                .capture(&root, source.clone(), "main.py", &files)
                .is_err(),
        ));
        results.push(("recheck_in_root_hard_link", good.recheck_source().is_err()));
        fs::remove_file(root.join("inner/alias.py")).unwrap();
        assert!(good.recheck_source().is_ok(), "control: hard link removed");
        // A FIFO is refused without blocking admission (O_NONBLOCK + S_ISREG).
        rustix::fs::mknodat(
            rustix::fs::CWD,
            root.join("inner/fifo"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let started = Instant::now();
        let fifo = policy.capture(
            &root,
            source.clone(),
            "main.py",
            &["main.py".into(), "inner/fifo".into()],
        );
        results.push((
            "capture_fifo_nonblocking",
            fifo.is_err() && started.elapsed() < Duration::from_secs(2),
        ));
        fs::remove_file(root.join("inner/fifo")).unwrap();
        // Replacing the approved root directory itself is not the approved root.
        fs::rename(&root, t.path().join("project-old")).unwrap();
        fs::create_dir_all(root.join("inner")).unwrap();
        fs::copy(t.path().join("project-old/main.py"), root.join("main.py")).unwrap();
        fs::write(root.join("inner/payload.txt"), ALLOWED).unwrap();
        results.push(("recheck_replaced_root", good.recheck_source().is_err()));
        results.push((
            "capture_replaced_root",
            policy.capture(&root, source, "main.py", &files).is_err(),
        ));
        println!("STAGE4_REPAIR_B2_STATIC {results:?}");
        assert!(results.iter().all(|(_, denied)| *denied), "{results:?}");
    }
    /// Generated code may write at most the 16 MiB /tmp tmpfs anywhere; every other
    /// mount (/, /dev, /dev/shm, /work, runtime) is read-only. SysV IPC objects
    /// (memory outside RLIMIT_AS) and nested user namespaces are denied.
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "needs the Linux isolation backend (non-Linux fails closed before any spawn)"
    )]
    fn repair_b4_writable_capacity_bounded_and_nested_userns_denied() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "repair_b4_writable_capacity_bounded_and_nested_userns_denied",
        ) {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        let code: &[u8] = br#"import os,errno,json,ctypes
CAP=17*1024*1024
r={'writable':{},'statvfs':{}}
def fill(d):
    n=0
    try:
        while n*4096<CAP:
            p=os.path.join(d,'pai-cap-%d'%n)
            fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
            try:
                if os.write(fd,b'x'*4096)!=4096: break
            finally: os.close(fd)
            n+=1
    except OSError: pass
    for i in range(n+1):
        try: os.unlink(os.path.join(d,'pai-cap-%d'%i))
        except OSError: pass
    return n*4096
seen=0
for d,ds,fs in os.walk('/',followlinks=False):
    seen+=1
    b=fill(d)
    if b: r['writable'][d]=b
for d in ['/','/dev','/dev/shm','/tmp','/work','/usr']:
    s=os.statvfs(d); r['statvfs'][d]=[s.f_blocks*s.f_frsize,bool(s.f_flag&1)]
libc=ctypes.CDLL(None,use_errno=True)
def call(f,*a):
    rc=f(*a)
    return 0 if rc>=0 else errno.errorcode.get(ctypes.get_errno(),'?')
r['shmget']=call(libc.shmget,0,1<<20,0o1600)
r['msgget']=call(libc.msgget,0,0o1600)
r['semget']=call(libc.semget,0,1,0o1600)
with open('/dev/null','wb') as f: f.write(b'x')
r['dev_null']=len(open('/dev/zero','rb').read(8))+len(os.urandom(8))
r['dirs']=seen
r['userns']=call(libc.unshare,0x10000000)
print(json.dumps(r,separators=(',',':')))
"#;
        fs::write(root.join("main.py"), code).unwrap();
        let policy = SnapshotPolicy::new(vec![root.clone()], temp.path().to_owned()).unwrap();
        let source = TrustedSource {
            source_id: "synthetic".into(),
            source_version: "1".into(),
            source_commit: "a".repeat(40),
            file_digest: hash(code),
        };
        let snapshot = policy
            .capture(&root, source, "main.py", &["main.py".into()])
            .unwrap();
        let backend =
            LinuxIsolation::new().expect("real isolation is a test precondition, never skip");
        let limits = RunLimits {
            cpu_seconds: 5,
            memory_bytes: 128 * 1024 * 1024,
            processes: 32,
            timeout_ms: 5000,
            output_bytes: 4096,
        };
        let argv = vec![PYTHON.into(), "-I".into(), "/work/main.py".into()];
        let run = backend
            .run(&snapshot, &argv, &[], &limits, &Cancellation::default())
            .unwrap();
        let stdout: Vec<u8> = (0..run.stdout_hex.len() / 2)
            .map(|i| u8::from_str_radix(&run.stdout_hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        println!(
            "STAGE4_REPAIR_B4 status={:?} termination={:?} stdout={} stderr_hex_len={}",
            run.status,
            run.termination,
            String::from_utf8_lossy(&stdout).trim(),
            run.stderr_hex.len()
        );
        assert_eq!(run.termination, Termination::Completed);
        assert_eq!(run.status, Some(0));
        let v: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let writable = v["writable"].as_object().unwrap();
        let total: u64 = writable.values().map(|b| b.as_u64().unwrap()).sum();
        assert_eq!(
            writable.keys().collect::<Vec<_>>(),
            vec!["/tmp"],
            "{writable:?}"
        );
        assert!(total <= WRITABLE_TMPFS_BYTES, "wrote {total} bytes");
        assert!(
            v["dirs"].as_u64().unwrap() > 100,
            "walk did not cover the sandbox"
        );
        for d in ["/", "/dev", "/dev/shm", "/work", "/usr"] {
            assert_eq!(
                v["statvfs"][d][1],
                serde_json::Value::Bool(true),
                "{d} writable"
            );
        }
        assert_eq!(v["statvfs"]["/tmp"][0], WRITABLE_TMPFS_BYTES);
        assert_eq!(v["statvfs"]["/tmp"][1], serde_json::Value::Bool(false));
        for call in ["shmget", "msgget", "semget"] {
            assert_eq!(v[call], "EPERM", "{call} allowed");
        }
        assert_ne!(v["userns"], 0, "nested user namespace allowed");
        assert_eq!(v["dev_null"], 16);
    }
}
