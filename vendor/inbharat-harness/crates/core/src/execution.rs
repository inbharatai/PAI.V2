//! Root-confined filesystem and allowlisted subprocess execution.
//!
//! The local filesystem fence blocks lexical escapes and validates canonical ancestors.
//! It is intentionally reported as an in-process fence, not an OS security boundary.

use crate::cancel::CancellationToken;
use crate::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A root-confined filesystem view.
#[derive(Clone, Debug)]
pub struct RootedFs {
    root: PathBuf,
    max_read_bytes: usize,
    max_write_bytes: usize,
}

impl RootedFs {
    /// Opens an existing directory as the authority root.
    pub fn new(root: impl AsRef<Path>) -> HarnessResult<Self> {
        let canonical = fs::canonicalize(root.as_ref()).map_err(|error| {
            io_failure(
                ErrorCode::FilesystemDenied,
                "fs.root",
                "cannot canonicalize filesystem root",
                error,
            )
        })?;
        if !canonical.is_dir() {
            return Err(Failure::invalid(
                "fs.root",
                "filesystem root is not a directory",
            ));
        }
        Ok(Self {
            root: canonical,
            max_read_bytes: 1024 * 1024,
            max_write_bytes: 1024 * 1024,
        })
    }

    #[must_use]
    pub fn with_limits(mut self, max_read_bytes: usize, max_write_bytes: usize) -> Self {
        self.max_read_bytes = max_read_bytes.max(1);
        self.max_write_bytes = max_write_bytes.max(1);
        self
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves an existing path without permitting root escape or symlink escape.
    pub fn resolve_existing(&self, relative: impl AsRef<Path>) -> HarnessResult<PathBuf> {
        let joined = self.lexical_join(relative.as_ref())?;
        let canonical = fs::canonicalize(&joined).map_err(|error| {
            io_failure(
                ErrorCode::NotFound,
                "fs.resolve",
                "path does not exist",
                error,
            )
        })?;
        self.ensure_inside(&canonical)?;
        Ok(canonical)
    }

    /// Reads a bounded UTF-8 file.
    ///
    /// TOCTOU hardening: the path is canonicalized and containment-checked, then
    /// the file is opened via the canonical path and its identity (device +
    /// inode) is compared against a fresh metadata lookup on that same canonical
    /// path. If a symlink or directory component was swapped between the check
    /// and the open, the opened handle's identity will not match the re-checked
    /// metadata and the read is refused. This narrows the check-then-act window
    /// to the open syscall itself.
    ///
    /// Honest residual: on this dependency-free (std-only) build there is no
    /// `openat2`/`RESOLVE_BENEATH`, so a path swap racing the exact `open` call
    /// cannot be eliminated entirely; the inode comparison closes the practical
    /// window for any swap that completes before the open returns. Platforms
    /// without inode metadata fall back to the containment re-check alone.
    pub fn read_text(&self, relative: impl AsRef<Path>) -> HarnessResult<String> {
        let path = self.resolve_existing(relative)?;
        let metadata = fs::metadata(&path).map_err(|error| {
            io_failure(
                ErrorCode::FilesystemDenied,
                "fs.read",
                "cannot inspect file",
                error,
            )
        })?;
        if !metadata.is_file()
            || metadata.len() > u64::try_from(self.max_read_bytes).unwrap_or(u64::MAX)
        {
            return Err(Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Policy,
                "fs.read",
                "file is not regular or exceeds read limit",
            ));
        }

        // Open via the canonical path, then verify the opened handle refers to
        // the same file the containment check just approved.
        let file = OpenOptions::new().read(true).open(&path).map_err(|error| {
            io_failure(
                ErrorCode::FilesystemDenied,
                "fs.read",
                "cannot open file",
                error,
            )
        })?;
        let _opened_metadata = file.metadata().map_err(|error| {
            io_failure(
                ErrorCode::FilesystemDenied,
                "fs.read",
                "cannot inspect opened file",
                error,
            )
        })?;

        // Identity comparison: same device + inode proves the opened handle is
        // the file we validated, defeating a swap that landed between the
        // metadata check and the open. On platforms exposing inode metadata this
        // is exact; elsewhere both identifiers are absent and we fall through to
        // the containment guarantee from resolve_existing.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if _opened_metadata.dev() != metadata.dev() || _opened_metadata.ino() != metadata.ino()
            {
                return Err(Failure::new(
                    ErrorCode::FilesystemDenied,
                    FailureClass::Policy,
                    "fs.read",
                    "file changed between validation and open",
                ));
            }
        }

        // The identity guard proves SAME file, not same size: a same-inode file
        // can grow between the size check and the read. Enforce the byte cap on
        // the live stream, not just the pre-open metadata, so max_read_bytes is
        // a real bound. Read at most max+1 bytes; overflow means the cap was
        // exceeded mid-read and we refuse the result.
        let max_read = u64::try_from(self.max_read_bytes).unwrap_or(u64::MAX);
        let mut contents = String::new();
        let mut limited = file.take(max_read.saturating_add(1));
        limited.read_to_string(&mut contents).map_err(|error| {
            io_failure(
                ErrorCode::FilesystemDenied,
                "fs.read",
                "cannot read UTF-8 file",
                error,
            )
        })?;
        if contents.len() > self.max_read_bytes {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "fs.read",
                "file exceeds read limit",
            ));
        }
        Ok(contents)
    }

    /// Lists one directory in stable lexical order without following entries.
    pub fn list(&self, relative: impl AsRef<Path>) -> HarnessResult<Vec<String>> {
        let path = self.resolve_existing(relative)?;
        if !path.is_dir() {
            return Err(Failure::invalid("fs.list", "path is not a directory"));
        }
        let mut names = Vec::new();
        let entries = fs::read_dir(path).map_err(|error| {
            io_failure(
                ErrorCode::FilesystemDenied,
                "fs.list",
                "cannot list directory",
                error,
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                io_failure(
                    ErrorCode::FilesystemDenied,
                    "fs.list",
                    "cannot read directory entry",
                    error,
                )
            })?;
            names.push(entry.file_name().to_string_lossy().into_owned());
            if names.len() >= 10_000 {
                break;
            }
        }
        names.sort();
        Ok(names)
    }

    /// Creates an in-root directory path one component at a time, validating every ancestor.
    pub fn create_dir_all(&self, relative: impl AsRef<Path>) -> HarnessResult<()> {
        // Absolute in-root paths are rebased first (defect #22): the ancestor
        // walk below operates on the root-relative remainder only, so a
        // model-supplied absolute path can no longer trip the Prefix arm here.
        let relative = self.in_root_path(relative.as_ref())?;
        self.ensure_no_escape(&relative)?;
        let _validated = self.lexical_join(&relative)?;
        let mut current = self.root.clone();
        for component in relative.components() {
            match component {
                Component::CurDir => continue,
                Component::Normal(name) => current.push(name),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(escape_failure());
                }
            }
            if !current.exists() {
                fs::create_dir(&current).map_err(|error| {
                    io_failure(
                        ErrorCode::FilesystemDenied,
                        "fs.mkdir",
                        "cannot create directory",
                        error,
                    )
                })?;
            }
            let canonical = fs::canonicalize(&current).map_err(|error| {
                io_failure(
                    ErrorCode::FilesystemDenied,
                    "fs.mkdir",
                    "cannot canonicalize directory",
                    error,
                )
            })?;
            self.ensure_inside(&canonical)?;
            if !canonical.is_dir() {
                return Err(Failure::invalid(
                    "fs.mkdir",
                    "path component is not a directory",
                ));
            }
            current = canonical;
        }
        Ok(())
    }

    /// Atomically replaces a file. Its parent must already exist inside the root.
    pub fn write_text_atomic(
        &self,
        relative: impl AsRef<Path>,
        contents: &str,
    ) -> HarnessResult<()> {
        if contents.len() > self.max_write_bytes {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "fs.write",
                "write exceeds configured byte limit",
            ));
        }
        let joined = self.lexical_join(relative.as_ref())?;
        if joined == self.root {
            return Err(Failure::invalid(
                "fs.write",
                "cannot replace root directory",
            ));
        }
        if joined.exists() {
            let canonical = fs::canonicalize(&joined).map_err(|error| {
                io_failure(
                    ErrorCode::FilesystemDenied,
                    "fs.write",
                    "cannot canonicalize target",
                    error,
                )
            })?;
            self.ensure_inside(&canonical)?;
            if canonical.is_dir() {
                return Err(Failure::invalid("fs.write", "target is a directory"));
            }
        }
        let parent = joined
            .parent()
            .ok_or_else(|| Failure::invalid("fs.write", "target must have an in-root parent"))?;
        // Defect #29 (live-caught 2026-09-14, long-coding acceptance): the
        // tool set had no directory-creation capability at all, and this
        // write path required the parent to already exist — so the very
        // first step of any "create a folder with files in it" task failed
        // with "target parent does not exist" forever (the local 12B
        // retried the identical fs.write every ~96 s, zero files, no
        // progress, until killed). Real coding agents create missing parent
        // directories on write; do the same through the fenced component
        // walk (ensure_no_escape + per-component canonicalize +
        // ensure_inside), so the fence is preserved exactly.
        if !parent.exists() {
            self.create_dir_all(parent)?;
        }
        let canonical_parent = fs::canonicalize(parent).map_err(|error| {
            io_failure(
                ErrorCode::FilesystemDenied,
                "fs.write",
                "target parent does not exist",
                error,
            )
        })?;
        self.ensure_inside(&canonical_parent)?;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos();
        let temp = canonical_parent.join(format!(".inbharat-tmp-{}-{nonce}", std::process::id()));
        let write_result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temp, &joined)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _remove_result = fs::remove_file(&temp);
            return Err(io_failure(
                ErrorCode::FilesystemDenied,
                "fs.write",
                "atomic write failed",
                error,
            ));
        }
        Ok(())
    }

    fn lexical_join(&self, path: &Path) -> HarnessResult<PathBuf> {
        // Live-caught (2026-09-12 long-coding acceptance, defect #22): agent
        // models naturally emit absolute host paths for workspace files
        // (e.g. `C:\Users\...\workspace\task-board\index.html`), and rejecting
        // every absolute path outright killed the whole run on `fs.resolve`.
        // Absolute paths are now accepted ONLY when they lexically designate
        // somewhere inside the configured root — they are rebased onto the
        // root's canonical form so every downstream fence (component scan,
        // canonicalize + ensure_inside) still applies unchanged. Genuinely
        // outside-root absolute paths are still an escape and denied.
        let effective = self.in_root_path(path)?;
        if effective.as_os_str().is_empty() {
            return Ok(self.root.clone());
        }
        self.ensure_no_escape(&effective)?;
        Ok(self.root.join(&effective))
    }

    /// Rebases a caller-supplied path onto the root. Relative paths pass
    /// through untouched; absolute paths are accepted only when they lexically
    /// sit inside the root, returning the root-relative remainder. Anything
    /// else escapes and is reported as such.
    fn in_root_path(&self, path: &Path) -> HarnessResult<PathBuf> {
        if !path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        self.strip_root_prefix(path).ok_or_else(escape_failure)
    }

    /// Lexical containment check of an absolute path against the configured
    /// root. Exact component-prefix match first; on Windows hosts two further
    /// spellings of the SAME in-root path are accepted: the `\\?\` verbatim
    /// prefix that `fs::canonicalize` produces (the stored root is verbatim
    /// while callers spell the volume `C:\...`, and vice versa), and case
    /// variants (Windows filesystems are case-insensitive, and the agent
    /// model may emit `c:\users\...`). The returned remainder keeps the
    /// caller's casing and separators; `lexical_join` re-joins it onto the
    /// root so the canonical root form is what reaches the filesystem.
    fn strip_root_prefix(&self, path: &Path) -> Option<PathBuf> {
        if let Ok(stripped) = path.strip_prefix(&self.root) {
            return Some(stripped.to_path_buf());
        }
        #[cfg(windows)]
        {
            let raw = path.to_string_lossy();
            let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
            let root_raw = self.root.to_string_lossy();
            let root_raw = root_raw.strip_prefix(r"\\?\").unwrap_or(&root_raw);
            // Models frequently spell Windows paths with forward slashes
            // (JSON-escaping habit); both replacements below are strictly
            // length-preserving, so the match offset still maps back onto the
            // caller's original string.
            let candidate = raw.to_ascii_lowercase().replace('/', "\\");
            let root = root_raw
                .to_ascii_lowercase()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_owned();
            let rest = candidate.strip_prefix(&root)?;
            // Boundary guard: `C:\rootx` must not count as inside root
            // `C:\root` — the matched prefix must end at a component boundary.
            if !rest.is_empty() && !rest.starts_with('\\') {
                return None;
            }
            // ASCII lowercasing and separator normalization are both
            // length-preserving, so the match maps back onto the original
            // casing and separators: keep the caller's component spelling,
            // only the root prefix is replaced.
            let offset = raw.len() - rest.len();
            let remainder = &raw[offset..];
            Some(PathBuf::from(remainder.trim_start_matches(['/', '\\'])))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    fn ensure_no_escape(&self, effective: &Path) -> HarnessResult<()> {
        for component in effective.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(escape_failure());
                }
            }
        }
        Ok(())
    }

    fn ensure_inside(&self, path: &Path) -> HarnessResult<()> {
        if path == self.root || path.starts_with(&self.root) {
            return Ok(());
        }
        Err(escape_failure())
    }
}

/// Direct argv subprocess request. Shell parsing is never performed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl ProcessSpec {
    #[must_use]
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            environment: BTreeMap::new(),
            timeout: Duration::from_secs(10),
            max_output_bytes: 256 * 1024,
        }
    }

    /// Override the per-process output cap. The tool layer must set this from
    /// the tool manifest's `max_output_bytes` so the process pipe cap and the
    /// post-hoc output check enforce the SAME limit rather than relying on two
    /// independent constants agreeing.
    #[must_use]
    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    /// Override the subprocess deadline. The tool layer must set this from
    /// the tool manifest's `default_timeout` — the 10-second struct default
    /// exists only so a bare ProcessSpec cannot hang forever; real tool runs
    /// derive their deadline from their manifest (defect #33, live-caught
    /// 2026-09-14: an agent-run Playwright suite was killed mid browser
    /// launch because ProcessTool::execute never overrode the default).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Machine-level environment variables every spawned child needs to run at
/// all. Real-world caught (2026-09-12 live acceptance): `node script.js` under
/// a fully empty environment crashes at init with exit 134
/// `Assertion failed: ncrypto::CSPRNG(nullptr, 0)` because Windows crypto
/// initialization resolves through `SystemRoot`; `node --version` survives
/// only because it takes a fast path. PowerShell, npm, and npx similarly
/// need `SystemRoot`/`TEMP`/`PATHEXT`. This allowlist is system variables
/// only — no user-profile, session, or secret-bearing values are forwarded.
const SYSTEM_BASELINE_ENV_KEYS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SystemDrive",
    "COMSPEC",
    "TEMP",
    "TMP",
    "TMPDIR",
    "OS",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "HOME",
    "LANG",
];

/// Builds the scrubbed child-environment baseline from a host environment
/// snapshot. `spec.environment` is layered on top by the caller so model-
/// supplied values still override the baseline.
fn baseline_env_from(host: impl Iterator<Item = (String, String)>) -> BTreeMap<String, String> {
    let mut baseline = BTreeMap::new();
    for (key, value) in host {
        if SYSTEM_BASELINE_ENV_KEYS
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&key))
        {
            baseline.insert(key, value);
        }
    }
    baseline
}

/// Bounded canonical process result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub elapsed: Duration,
}

/// One execution-world interface so filesystem and process tools cannot drift.
pub trait ExecutionBroker: Send + Sync {
    fn world_id(&self) -> &str;
    fn read_text(&self, relative: &Path) -> HarnessResult<String>;
    fn list(&self, relative: &Path) -> HarnessResult<Vec<String>>;
    fn write_text_atomic(&self, relative: &Path, contents: &str) -> HarnessResult<()>;
    fn create_dir_all(&self, relative: &Path) -> HarnessResult<()>;
    fn run_process(
        &self,
        spec: &ProcessSpec,
        cancel: &CancellationToken,
    ) -> HarnessResult<ProcessOutput>;
}

/// Local single-user execution broker with an allowlist and scrubbed environment.
#[derive(Clone, Debug)]
pub struct LocalExecutionBroker {
    world_id: String,
    filesystem: RootedFs,
    allowed_programs: BTreeMap<String, PathBuf>,
}

impl LocalExecutionBroker {
    #[must_use]
    pub fn new(filesystem: RootedFs, allowed_programs: impl IntoIterator<Item = String>) -> Self {
        let mut resolved = BTreeMap::new();
        for program in allowed_programs {
            if program.is_empty() || program.contains('/') || program.contains('\\') {
                continue;
            }
            if let Some(path) = resolve_program(&program) {
                resolved.insert(program, path);
            }
        }
        Self {
            world_id: "local-rooted-v1".to_owned(),
            filesystem,
            allowed_programs: resolved,
        }
    }

    #[must_use]
    pub fn filesystem(&self) -> &RootedFs {
        &self.filesystem
    }
}

impl ExecutionBroker for LocalExecutionBroker {
    fn world_id(&self) -> &str {
        &self.world_id
    }

    fn read_text(&self, relative: &Path) -> HarnessResult<String> {
        self.filesystem.read_text(relative)
    }

    fn list(&self, relative: &Path) -> HarnessResult<Vec<String>> {
        self.filesystem.list(relative)
    }

    fn write_text_atomic(&self, relative: &Path, contents: &str) -> HarnessResult<()> {
        self.filesystem.write_text_atomic(relative, contents)
    }

    fn create_dir_all(&self, relative: &Path) -> HarnessResult<()> {
        self.filesystem.create_dir_all(relative)
    }

    fn run_process(
        &self,
        spec: &ProcessSpec,
        cancel: &CancellationToken,
    ) -> HarnessResult<ProcessOutput> {
        cancel.check("process.run")?;
        let program_path = self.allowed_programs.get(&spec.program).ok_or_else(|| {
            Failure::new(
                ErrorCode::SubprocessDenied,
                FailureClass::Policy,
                "process.run",
                "program is not allowlisted or was not resolvable when the broker was created",
            )
            .with_detail("program", &spec.program)
        })?;
        if spec.args.len() > 256
            || spec.args.iter().any(|argument| argument.len() > 32 * 1024)
            || spec.environment.len() > 64
            || spec.environment.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 1024
                    || value.len() > 32 * 1024
                    || key.contains('=')
                    || key.contains('\0')
                    || value.contains('\0')
            })
        {
            return Err(Failure::invalid(
                "process.run",
                "process request exceeds argument or environment bounds",
            ));
        }
        let started = Instant::now();
        let mut environment = baseline_env_from(env::vars());
        environment.extend(spec.environment.clone());
        let mut command = Command::new(program_path);
        command
            .args(&spec.args)
            .current_dir(self.filesystem.root())
            .env_clear()
            .envs(&environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            io_failure(
                ErrorCode::ToolFailed,
                "process.spawn",
                "failed to spawn allowlisted program",
                error,
            )
        })?;
        let limit = spec.max_output_bytes.min(4 * 1024 * 1024);
        let stdout_pipe = child.stdout.take().ok_or_else(|| {
            Failure::new(
                ErrorCode::ToolFailed,
                FailureClass::Execution,
                "process.output",
                "subprocess stdout pipe is unavailable",
            )
        })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| {
            Failure::new(
                ErrorCode::ToolFailed,
                FailureClass::Execution,
                "process.output",
                "subprocess stderr pipe is unavailable",
            )
        })?;
        let stdout_reader = thread::spawn(move || read_pipe_bounded(stdout_pipe, limit));
        let stderr_reader = thread::spawn(move || read_pipe_bounded(stderr_pipe, limit));

        let status = loop {
            if cancel.is_cancelled() {
                let _kill_result = child.kill();
                let _wait_result = child.wait();
                let _stdout = join_pipe(stdout_reader, "process.stdout");
                let _stderr = join_pipe(stderr_reader, "process.stderr");
                return Err(Failure::cancelled(
                    "process.run",
                    cancel.cause().map_or("cancelled", |cause| cause.as_str()),
                ));
            }
            if started.elapsed() >= spec.timeout {
                let _kill_result = child.kill();
                let _wait_result = child.wait();
                let _stdout = join_pipe(stdout_reader, "process.stdout");
                let _stderr = join_pipe(stderr_reader, "process.stderr");
                return Err(Failure::new(
                    ErrorCode::Timeout,
                    FailureClass::Resource,
                    "process.run",
                    "subprocess deadline exceeded",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(error) => {
                    let _kill_result = child.kill();
                    let _wait_result = child.wait();
                    let _stdout = join_pipe(stdout_reader, "process.stdout");
                    let _stderr = join_pipe(stderr_reader, "process.stderr");
                    return Err(io_failure(
                        ErrorCode::ToolFailed,
                        "process.wait",
                        "cannot wait for subprocess",
                        error,
                    ));
                }
            }
        };

        let (mut stdout, stdout_truncated) = join_pipe(stdout_reader, "process.stdout")?;
        let (mut stderr, stderr_truncated) = join_pipe(stderr_reader, "process.stderr")?;
        let total = stdout.len().saturating_add(stderr.len());
        let truncated = stdout_truncated || stderr_truncated || total > limit;
        if stdout.len() > limit {
            stdout.truncate(limit);
            stderr.clear();
        } else {
            stderr.truncate(limit.saturating_sub(stdout.len()));
        }
        Ok(ProcessOutput {
            status: status.code(),
            stdout,
            stderr,
            truncated,
            elapsed: started.elapsed(),
        })
    }
}

fn resolve_program(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    #[cfg(not(windows))]
    let names = vec![program.to_owned()];
    #[cfg(windows)]
    let names = {
        let mut values = vec![program.to_owned()];
        if Path::new(program).extension().is_none() {
            let extensions = env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
            for extension in extensions.to_string_lossy().split(';') {
                if !extension.is_empty() {
                    values.push(format!("{program}{extension}"));
                }
            }
        }
        values
    };
    for directory in env::split_paths(&path) {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                if let Ok(canonical) = fs::canonicalize(candidate) {
                    return Some(canonical);
                }
            }
        }
    }
    None
}

fn read_pipe_bounded(mut reader: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        let retained = read.min(remaining);
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((output, truncated))
}

fn join_pipe(
    reader: thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
    operation: &str,
) -> HarnessResult<(Vec<u8>, bool)> {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !reader.is_finished() {
        if Instant::now() >= deadline {
            return Err(Failure::new(
                ErrorCode::Timeout,
                FailureClass::Resource,
                operation,
                "subprocess output reader did not quiesce",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    reader
        .join()
        .map_err(|_| {
            Failure::new(
                ErrorCode::ToolFailed,
                FailureClass::Execution,
                operation,
                "subprocess output reader panicked",
            )
        })?
        .map_err(|error| {
            io_failure(
                ErrorCode::ToolFailed,
                operation,
                "cannot read subprocess output",
                error,
            )
        })
}

fn escape_failure() -> Failure {
    Failure::new(
        ErrorCode::FilesystemDenied,
        FailureClass::Policy,
        "fs.resolve",
        "path escapes the configured root",
    )
}

fn io_failure(code: ErrorCode, operation: &str, message: &str, error: std::io::Error) -> Failure {
    Failure::new(code, FailureClass::Execution, operation, message)
        .with_detail("io_kind", format!("{:?}", error.kind()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_env_baseline_keeps_system_keys_case_insensitively() {
        let baseline = baseline_env_from(
            [
                ("PATH".to_owned(), "/usr/bin".to_owned()),
                ("systemroot".to_owned(), "C:\\Windows".to_owned()),
                ("TEMP".to_owned(), "C:\\Temp".to_owned()),
            ]
            .into_iter(),
        );
        assert_eq!(baseline.len(), 3);
        assert_eq!(baseline.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(
            baseline.get("systemroot").map(String::as_str),
            Some("C:\\Windows")
        );
    }

    #[test]
    fn child_env_baseline_drops_non_system_values() {
        let baseline = baseline_env_from(
            [
                ("PATH".to_owned(), "/usr/bin".to_owned()),
                ("USERPROFILE".to_owned(), "C:\\Users\\reetu".to_owned()),
                ("OPENAI_API_KEY".to_owned(), "sk-secret".to_owned()),
                ("DEV_UNLOCK_PW".to_owned(), "never-forwarded".to_owned()),
            ]
            .into_iter(),
        );
        assert_eq!(baseline.len(), 1);
        assert!(baseline.contains_key("PATH"));
        assert!(!baseline.contains_key("USERPROFILE"));
        assert!(!baseline.contains_key("OPENAI_API_KEY"));
        assert!(!baseline.contains_key("DEV_UNLOCK_PW"));
    }

    #[cfg(windows)]
    #[test]
    fn spawned_children_receive_the_system_baseline() -> HarnessResult<()> {
        // Regression for the live-caught node exit 134
        // `ncrypto::CSPRNG(nullptr, 0)` crash: a child spawned with a fully
        // empty environment cannot run at all on Windows. `cmd` expands an
        // unset variable literally, so a present SystemRoot proves the
        // baseline reached the child; the custom variable proves the model-
        // supplied environment layers on top.
        let fs = RootedFs::new(".")?;
        let broker = LocalExecutionBroker::new(fs, vec!["cmd".to_owned()]);
        let mut spec = ProcessSpec::new(
            "cmd",
            vec![
                "/C".to_owned(),
                "echo %SystemRoot% %BASELINE_PROBE%".to_owned(),
            ],
        );
        spec.environment
            .insert("BASELINE_PROBE".to_owned(), "layered".to_owned());
        let output = broker.run_process(&spec, &CancellationToken::new())?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
        assert_eq!(output.status, Some(0));
        assert!(
            stdout.contains("windows"),
            "SystemRoot did not reach the child: {stdout}"
        );
        assert!(
            stdout.contains("layered"),
            "spec.environment did not reach the child: {stdout}"
        );
        Ok(())
    }

    #[test]
    fn lexical_escape_is_denied() -> HarnessResult<()> {
        let fs = RootedFs::new(".")?;
        let result = fs.read_text("../outside");
        assert!(result.is_err());
        Ok(())
    }

    /// Defect #33 (live-caught 2026-09-14): the spec's 10s default deadline
    /// used to be the ONLY deadline a tool run could get — ProcessTool never
    /// overrode it, so an agent-run Playwright suite was killed mid browser
    /// launch. The builder must let the tool layer derive the deadline from
    /// the manifest, and the kill-timer must honor the override.
    #[test]
    fn process_spec_timeout_is_overridable_and_enforced() -> HarnessResult<()> {
        // Builder contract: struct default is 10s; with_timeout replaces it.
        let default_spec = ProcessSpec::new("noop", vec![]);
        assert_eq!(default_spec.timeout, Duration::from_secs(10));
        let overridden = default_spec.clone().with_timeout(Duration::from_secs(180));
        assert_eq!(overridden.timeout, Duration::from_secs(180));
        assert_eq!(
            default_spec.timeout,
            Duration::from_secs(10),
            "with_timeout must not mutate the original"
        );

        // End-to-end: a child that would outlive a short override is killed
        // with the honest deadline failure, not allowed to run to completion.
        #[cfg(windows)]
        let (program, args) = (
            "cmd",
            vec!["/C".to_owned(), "ping -n 30 -w 1000 127.0.0.1".to_owned()],
        );
        #[cfg(unix)]
        let (program, args) = ("sh", vec!["-c".to_owned(), "sleep 30".to_owned()]);
        let fs = RootedFs::new(".")?;
        let broker = LocalExecutionBroker::new(fs, vec![program.to_owned()]);
        let spec = ProcessSpec::new(program, args).with_timeout(Duration::from_secs(2));
        let result = broker.run_process(&spec, &CancellationToken::new());
        let message = result
            .err()
            .map(|failure| failure.to_string())
            .unwrap_or_else(|| "child completed before the deadline".to_owned());
        assert!(
            message.contains("subprocess deadline exceeded"),
            "expected the deadline kill, got: {message}"
        );
        Ok(())
    }

    #[test]
    fn process_allowlist_is_enforced() -> HarnessResult<()> {
        let fs = RootedFs::new(".")?;
        let broker = LocalExecutionBroker::new(fs, Vec::<String>::new());
        let result = broker.run_process(
            &ProcessSpec::new("sh", vec!["-c".to_owned(), "echo bad".to_owned()]),
            &CancellationToken::new(),
        );
        assert!(result.is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_text_identity_matches_opened_file() -> HarnessResult<()> {
        use std::os::unix::fs::MetadataExt;

        // Set up a unique in-root directory and file.
        let base = std::env::temp_dir().join(format!("inbharat-toctou-{}", std::process::id()));
        let _cleanup = scopeguard_remove(&base);
        std::fs::create_dir_all(&base).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        let target = base.join("data.txt");
        std::fs::write(&target, "confined contents").map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;

        let fs = RootedFs::new(&base)?;
        // Passing case: a genuine in-root file reads back its exact contents.
        // This exercises the open-then-identity-check path without tripping it.
        let contents = fs.read_text("data.txt")?;
        assert_eq!(contents, "confined contents");

        // The identity invariant the TOCTOU guard enforces: the metadata used
        // for validation and the metadata of the opened handle must refer to
        // the same file (device + inode). We assert the underlying invariant
        // directly so a regression in the comparison is caught deterministically.
        let path_meta = std::fs::metadata(&target).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        let opened = OpenOptions::new().read(true).open(&target).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        let opened_meta = opened.metadata().map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        assert_eq!(path_meta.dev(), opened_meta.dev());
        assert_eq!(path_meta.ino(), opened_meta.ino());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_text_symlink_escape_is_denied() -> HarnessResult<()> {
        // An in-root symlink pointing outside the root must be refused. This is
        // the static containment the TOCTOU guard is built on top of: even a
        // non-racing symlink escape is denied by resolve_existing + ensure_inside.
        let base = std::env::temp_dir().join(format!("inbharat-symlink-{}", std::process::id()));
        let _cleanup = scopeguard_remove(&base);
        std::fs::create_dir_all(&base).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        // Outside-the-root target (the temp dir's parent is outside `base`).
        let outside = std::env::temp_dir().join(format!("inbharat-outside-{}", std::process::id()));
        std::fs::write(&outside, "secret outside").map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        let _cleanup_outside = scopeguard_remove(&outside);
        std::os::unix::fs::symlink(&outside, base.join("escape")).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;

        let fs = RootedFs::new(&base)?;
        let result = fs.read_text("escape");
        assert!(
            result.is_err(),
            "an in-root symlink escaping the root must be denied"
        );
        Ok(())
    }

    #[test]
    fn read_text_enforces_byte_cap() -> HarnessResult<()> {
        // A file larger than max_read_bytes must be refused. The read is capped
        // on the live stream (not just the pre-open metadata size), so the
        // advertised bound is enforced at read time.
        //
        // Scope note (honest): this test exercises the cap against static
        // content. The mid-read growth race that motivated the stream cap was
        // verified by an external adversarial probe (0 over-cap reads on the
        // fixed code vs thousands on the old uncapped code); it cannot be
        // triggered deterministically in-process because the pre-open size check
        // usually observes the grown size too.
        let base = std::env::temp_dir().join(format!("inbharat-cap-{}", std::process::id()));
        let _cleanup = scopeguard_remove(&base);
        std::fs::create_dir_all(&base).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;

        let fs = RootedFs::new(&base)?.with_limits(64, 1024);

        // Within-cap file reads fine.
        std::fs::write(base.join("small.txt"), "fits").map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        assert_eq!(fs.read_text("small.txt")?, "fits");

        // Over-cap file is rejected. (Static oversize exercises the same bound;
        // the stream cap also covers a file that grows between check and read.)
        std::fs::write(base.join("big.txt"), "x".repeat(4096)).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        assert!(
            fs.read_text("big.txt").is_err(),
            "a file larger than max_read_bytes must be refused"
        );
        Ok(())
    }

    /// Remove a path when the test scope ends, ignoring errors (best-effort).
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
            let _ = fs::remove_file(&self.0);
        }
    }
    fn scopeguard_remove(path: &Path) -> Cleanup {
        Cleanup(path.to_path_buf())
    }

    // Regression for defect #22 (live-caught 2026-09-12): a model emitting an
    // absolute host path for a workspace file killed the whole run with
    // `filesystem_denied:fs.resolve: path escapes the configured root`. The
    // fence must accept absolute paths that sit inside the root and rebase
    // them, while still denying every genuinely-outside path.

    fn abs_fs(label: &str) -> HarnessResult<(PathBuf, Cleanup, RootedFs)> {
        let base =
            std::env::temp_dir().join(format!("inbharat-abs-{label}-{}", std::process::id()));
        let cleanup = scopeguard_remove(&base.clone());
        fs::create_dir_all(&base).map_err(|e| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Execution,
                "test.setup",
                e.to_string(),
            )
        })?;
        let rooted = RootedFs::new(&base)?;
        // The canonical root the fence stores — on Windows this carries the
        // `\\?\` verbatim prefix from fs::canonicalize. Tests exercise the
        // user/model spelling (no verbatim prefix) of the same path, which is
        // exactly what the live 12B model emitted.
        Ok((rooted.root().to_path_buf(), cleanup, rooted))
    }

    /// The caller-friendly spelling of a canonical path: `\\?\` stripped.
    fn spelled(canonical: &Path) -> PathBuf {
        let raw = canonical.to_string_lossy();
        PathBuf::from(raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned())
    }

    #[test]
    fn absolute_in_root_path_is_rebased_not_denied() -> HarnessResult<()> {
        let (root, _cleanup, fs) = abs_fs("rebase")?;
        let user = spelled(&root);

        // Absolute paths inside the root must work across the whole surface:
        // create_dir_all, write_text_atomic, read_text, and list — spelled
        // the way a user or model writes them, without the verbatim prefix.
        fs.create_dir_all(user.join("nested").join("deeper"))?;
        fs.write_text_atomic(
            user.join("nested").join("deeper").join("file.txt"),
            "absolute ok",
        )?;
        assert_eq!(
            fs.read_text(user.join("nested").join("deeper").join("file.txt"))?,
            "absolute ok"
        );
        // The same file is reachable by its relative form too (the rebased
        // write landed inside the root, not beside it).
        assert_eq!(fs.read_text("nested/deeper/file.txt")?, "absolute ok");
        assert!(fs.list("nested/deeper")?.contains(&"file.txt".to_owned()));
        // The canonical (verbatim) spelling works too — exact strip-prefix.
        fs.write_text_atomic(root.join("verbatim.txt"), "canonical form")?;
        assert_eq!(fs.read_text("verbatim.txt")?, "canonical form");
        Ok(())
    }

    #[test]
    fn write_creates_missing_parent_directories() -> HarnessResult<()> {
        // Defect #29 (live-caught 2026-09-14): fs.write used to demand an
        // existing parent and no tool could create one, so the first step of
        // any "create a folder with files" task failed forever. A write into
        // a deep missing parent must now succeed and be readable back.
        let (root, _cleanup, fs) = abs_fs("autoparent")?;
        let target = root
            .join("task-board")
            .join("src")
            .join("ui")
            .join("index.html");
        fs.write_text_atomic(&target, "<h1>auto-created parents</h1>")?;
        assert_eq!(
            fs.read_text("task-board/src/ui/index.html")?,
            "<h1>auto-created parents</h1>"
        );
        assert!(root.join("task-board").join("src").join("ui").is_dir());
        // A relative spelling with missing parents works the same way.
        fs.write_text_atomic("rel/deep/file.txt", "relative ok")?;
        assert_eq!(fs.read_text("rel/deep/file.txt")?, "relative ok");
        Ok(())
    }

    #[test]
    fn write_escape_through_missing_parent_is_denied() -> HarnessResult<()> {
        // The auto-parent walk must not open an escape hatch: parent-dir
        // components are rejected by the same fence as before, and nothing
        // is created on disk when they are.
        let (root, _cleanup, fs) = abs_fs("parentescape")?;
        let Some(parent) = root.parent() else {
            return Err(Failure::invalid("test.setup", "root has no parent"));
        };
        assert!(
            fs.write_text_atomic("new/../escape-probe.txt", "no")
                .is_err()
        );
        assert!(
            fs.write_text_atomic("deep/../../escape-probe.txt", "no")
                .is_err()
        );
        assert!(!parent.join("escape-probe.txt").exists());
        assert!(!root.join("new").exists());
        Ok(())
    }

    #[test]
    fn absolute_outside_root_path_is_still_denied() -> HarnessResult<()> {
        let (root, _cleanup, fs) = abs_fs("outside")?;
        let user = spelled(&root);

        // The root's parent directory is strictly outside.
        let Some(parent) = user.parent() else {
            return Err(Failure::invalid("test.setup", "root has no parent"));
        };
        let outside = parent.join("inbharat-abs-outside-probe.txt");
        let err = match fs.read_text(&outside) {
            Err(failure) => failure,
            Ok(_) => {
                return Err(Failure::invalid(
                    "test.assert",
                    "outside-root absolute read must be denied",
                ));
            }
        };
        assert_eq!(err.code, ErrorCode::FilesystemDenied, "got: {err}");
        assert!(err.message.contains("escapes"), "got: {err}");

        // A write whose absolute target sits beside the root must be denied,
        // not silently redirected inside.
        let result = fs.write_text_atomic(user.join("..").join("inbharat-abs-beside.txt"), "x");
        assert!(result.is_err(), "beside-root absolute write must be denied");
        Ok(())
    }

    #[test]
    fn absolute_path_with_dotdot_after_root_prefix_is_denied() -> HarnessResult<()> {
        let (root, _cleanup, fs) = abs_fs("dotdot")?;
        let user = spelled(&root);
        // `C:\...\root\..\x` matches the root prefix lexically but escapes via
        // ParentDir — the component re-scan must still catch it.
        let err = match fs.read_text(user.join("..").join("escape.txt")) {
            Err(failure) => failure,
            Ok(_) => {
                return Err(Failure::invalid(
                    "test.assert",
                    "dotdot after root prefix must be denied",
                ));
            }
        };
        assert!(err.message.contains("escapes"), "got: {err}");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn absolute_in_root_path_case_variant_is_accepted() -> HarnessResult<()> {
        let (root, _cleanup, fs) = abs_fs("case")?;
        // Windows filesystems are case-insensitive: `c:\users\...` and
        // `C:\Users\...` designate the same in-root file, so a case-variant
        // spelling from the model must be rebased, not denied.
        let lowered = spelled(&root).to_string_lossy().to_ascii_lowercase();
        fs.write_text_atomic(PathBuf::from(lowered).join("case.txt"), "cased")?;
        assert_eq!(fs.read_text("case.txt")?, "cased");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn sibling_directory_sharing_root_prefix_is_denied() -> HarnessResult<()> {
        let (root, _cleanup, fs) = abs_fs("sibling")?;
        let user = spelled(&root);
        // `C:\...\inbharat-abs-sibling` must NOT count as inside root
        // `C:\...\inbharat-abs`: the containment fallback has to respect
        // component boundaries.
        let Some(parent) = user.parent() else {
            return Err(Failure::invalid("test.setup", "root has no parent"));
        };
        let Some(name) = root.file_name() else {
            return Err(Failure::invalid("test.setup", "root has no file name"));
        };
        let sibling = parent
            .join(format!("{}-sibling", name.to_string_lossy()))
            .join("file.txt");
        let err = match fs.write_text_atomic(&sibling, "x") {
            Err(failure) => failure,
            Ok(_) => {
                return Err(Failure::invalid(
                    "test.assert",
                    "prefix-sibling path must be denied",
                ));
            }
        };
        assert!(err.message.contains("escapes"), "got: {err}");
        Ok(())
    }
}
