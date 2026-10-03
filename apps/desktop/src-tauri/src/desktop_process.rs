//! Session-only consent and ownership for host commands. This is NOT a
//! filesystem or network sandbox. Folder grants authorize file tools only.
use inbharat_harness_core::execution::{DetachedSpawn, ProcessOutput};
use inbharat_harness_core::{
    CancellationToken, ErrorCode, Failure, FailureClass, HarnessResult, ProcessSpec,
};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Session {
    generation: u64,
    enabled: bool,
    processes: Vec<Arc<Mutex<OwnedProcess>>>,
}

#[derive(Clone, Default)]
pub(crate) struct DesktopProcessState(Arc<Mutex<Session>>);

#[derive(Clone)]
pub(crate) struct ProcessLease {
    state: DesktopProcessState,
    generation: u64,
    enabled: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct ExecutionStatus {
    pub enabled: bool,
    pub owned_processes: usize,
}

fn denied(message: &str) -> Failure {
    Failure::new(
        ErrorCode::SubprocessDenied,
        FailureClass::Policy,
        "desktop.process",
        message,
    )
}

impl DesktopProcessState {
    pub(crate) fn lease(&self) -> ProcessLease {
        let session = self.0.lock().unwrap_or_else(|e| e.into_inner());
        ProcessLease {
            state: self.clone(),
            generation: session.generation,
            enabled: session.enabled,
        }
    }

    pub(crate) fn status(&self) -> ExecutionStatus {
        let session = self.0.lock().unwrap_or_else(|e| e.into_inner());
        ExecutionStatus {
            enabled: session.enabled,
            owned_processes: session.processes.len(),
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        // Hold admission closed until every old process has been terminated.
        let mut session = self.0.lock().unwrap_or_else(|e| e.into_inner());
        session.enabled = false;
        session.generation = session.generation.wrapping_add(1);
        for process in session.processes.drain(..) {
            process
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .terminate();
        }
        session.enabled = enabled;
    }
}

pub(crate) struct DesktopProcessBroker {
    root: PathBuf,
    programs: BTreeMap<String, PathBuf>,
    lease: ProcessLease,
}

impl DesktopProcessBroker {
    pub(crate) fn new(
        root: &Path,
        programs: impl IntoIterator<Item = String>,
        lease: ProcessLease,
    ) -> Self {
        Self {
            root: root.to_owned(),
            programs: programs
                .into_iter()
                .filter_map(|name| resolve_program(&name).map(|path| (name, path)))
                .collect(),
            lease,
        }
    }

    fn spawn(
        &self,
        spec: &ProcessSpec,
        cancel: &CancellationToken,
        capture: bool,
    ) -> HarnessResult<Arc<Mutex<OwnedProcess>>> {
        cancel.check("desktop.process")?;
        let mut session = self
            .lease
            .state
            .0
            .lock()
            .map_err(|_| denied("Command permission state is unavailable"))?;
        if !self.lease.enabled || !session.enabled || session.generation != self.lease.generation {
            return Err(denied("Host commands require explicit permission in Settings. Folder grants do not authorize commands. Restart the task after granting permission."));
        }
        if session.processes.len() >= 32 {
            return Err(denied(
                "32 owned processes are already active. Revoke command permission to stop them.",
            ));
        }
        let program = self
            .programs
            .get(&spec.program)
            .ok_or_else(|| denied("Program is not allowlisted or available"))?;
        if spec.args.len() > 256
            || spec
                .args
                .iter()
                .any(|arg| arg.len() > 32 * 1024 || arg.contains('\0'))
            || spec.environment.len() > 64
            || spec.environment.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 1024
                    || key.contains(['=', '\0'])
                    || value.len() > 32 * 1024
                    || value.contains('\0')
            })
        {
            return Err(Failure::invalid(
                "desktop.process",
                "Argument or environment bounds exceeded",
            ));
        }
        let mut command = Command::new(program);
        command
            .args(&spec.args)
            .current_dir(&self.root)
            .env_clear()
            .stdin(Stdio::null());
        // Deliberately omit user credentials and arbitrary host environment.
        for (key, value) in std::env::vars() {
            if [
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
            ]
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&key))
            {
                command.env(key, value);
            }
        }
        command.envs(&spec.environment);
        command.stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        command.stderr(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|e| Failure::invalid("desktop.process.spawn", e.to_string()))?;
        let tree = match ProcessTree::attach(&child) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::invalid(
                    "desktop.process.ownership",
                    error.to_string(),
                ));
            }
        };
        let process = Arc::new(Mutex::new(OwnedProcess {
            child,
            tree: Some(tree),
        }));
        session.processes.push(Arc::clone(&process));
        Ok(process)
    }

    pub(crate) fn spawn_detached(
        &self,
        spec: &ProcessSpec,
        cancel: &CancellationToken,
    ) -> HarnessResult<DetachedSpawn> {
        let process = self.spawn(spec, cancel, false)?;
        let pid = process
            .lock()
            .map_err(|_| denied("Process ownership lock failed"))?
            .child
            .id();
        Ok(DetachedSpawn { pid: Some(pid) })
    }

    pub(crate) fn run_process(
        &self,
        spec: &ProcessSpec,
        cancel: &CancellationToken,
    ) -> HarnessResult<ProcessOutput> {
        let started = Instant::now();
        let process = self.spawn(spec, cancel, true)?;
        let limit = spec.max_output_bytes.min(4 * 1024 * 1024);
        let (stdout, stderr) = {
            let mut owned = process.lock().unwrap_or_else(|e| e.into_inner());
            (
                owned.child.stdout.take().expect("captured stdout"),
                owned.child.stderr.take().expect("captured stderr"),
            )
        };
        let out_reader = thread::spawn(move || read_bounded(stdout, limit));
        let err_reader = thread::spawn(move || read_bounded(stderr, limit));
        let result = loop {
            if cancel.is_cancelled() {
                break Err(Failure::cancelled("desktop.process", "cancelled"));
            }
            if started.elapsed() >= spec.timeout {
                break Err(Failure::new(
                    ErrorCode::Timeout,
                    FailureClass::Resource,
                    "desktop.process",
                    "Command deadline exceeded",
                ));
            }
            match process
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .child
                .try_wait()
            {
                Ok(Some(status)) => break Ok(status.code()),
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(e) => break Err(Failure::invalid("desktop.process.wait", e.to_string())),
            }
        };
        // Kill descendants BEFORE joining pipe readers: a background child may
        // have inherited stdout even though its foreground parent has exited.
        process
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .terminate();
        self.lease
            .state
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .processes
            .retain(|entry| !Arc::ptr_eq(entry, &process));
        let out = out_reader
            .join()
            .map_err(|_| Failure::invalid("desktop.process.stdout", "Reader failed"))?;
        let err = err_reader
            .join()
            .map_err(|_| Failure::invalid("desktop.process.stderr", "Reader failed"))?;
        let (mut stdout, out_truncated) =
            out.map_err(|e| Failure::invalid("desktop.process.stdout", e.to_string()))?;
        let (mut stderr, err_truncated) =
            err.map_err(|e| Failure::invalid("desktop.process.stderr", e.to_string()))?;
        let truncated =
            out_truncated || err_truncated || stdout.len().saturating_add(stderr.len()) > limit;
        stdout.truncate(limit);
        stderr.truncate(limit.saturating_sub(stdout.len()));
        Ok(ProcessOutput {
            status: result?,
            stdout,
            stderr,
            truncated,
            elapsed: started.elapsed(),
        })
    }
}

fn read_bounded(mut pipe: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            return Ok((bytes, truncated));
        }
        let retain = count.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..retain]);
        truncated |= retain < count;
    }
}

fn resolve_program(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    let mut names = vec![name.to_owned()];
    if cfg!(windows) && Path::new(name).extension().is_none() {
        names.extend([".exe", ".com", ".cmd", ".bat"].map(|ext| format!("{name}{ext}")));
    }
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

struct OwnedProcess {
    child: Child,
    tree: Option<ProcessTree>,
}
impl OwnedProcess {
    fn terminate(&mut self) {
        if let Some(tree) = self.tree.take() {
            tree.terminate();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
struct ProcessTree(i32);
#[cfg(unix)]
impl ProcessTree {
    fn attach(child: &Child) -> std::io::Result<Self> {
        Ok(Self(child.id() as i32))
    }
    fn terminate(&self) {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // A negative PID addresses the dedicated process group created at spawn.
        // Host commands are trusted: a program can intentionally leave its group.
        unsafe {
            kill(-self.0, 9);
        }
    }
}

#[cfg(windows)]
struct ProcessTree(usize);
#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle()) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Self(job as usize))
        }
    }
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0 as _, 1);
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn broker(state: &DesktopProcessState) -> DesktopProcessBroker {
        DesktopProcessBroker::new(
            &std::env::temp_dir(),
            [if cfg!(windows) { "cmd" } else { "sh" }.to_owned()],
            state.lease(),
        )
    }
    fn echo() -> ProcessSpec {
        if cfg!(windows) {
            ProcessSpec::new("cmd", vec!["/C".into(), "echo hello".into()])
        } else {
            ProcessSpec::new("sh", vec!["-c".into(), "printf hello".into()])
        }
    }
    #[test]
    fn default_denies_both_process_lanes() {
        let state = DesktopProcessState::default();
        let broker = broker(&state);
        assert!(broker
            .run_process(&echo(), &CancellationToken::new())
            .is_err());
        assert!(broker
            .spawn_detached(&echo(), &CancellationToken::new())
            .is_err());
    }
    #[test]
    fn old_run_cannot_acquire_a_later_permission() {
        let state = DesktopProcessState::default();
        let old = broker(&state);
        state.set_enabled(true);
        assert!(old.run_process(&echo(), &CancellationToken::new()).is_err());
        let current = broker(&state);
        assert!(String::from_utf8(
            current
                .run_process(&echo(), &CancellationToken::new())
                .unwrap()
                .stdout
        )
        .unwrap()
        .contains("hello"));
        state.set_enabled(false);
        state.set_enabled(true);
        assert!(current
            .run_process(&echo(), &CancellationToken::new())
            .is_err());
    }
    #[cfg(unix)]
    #[test]
    fn foreground_parent_exit_does_not_hang_on_descendant_pipes() {
        let state = DesktopProcessState::default();
        state.set_enabled(true);
        let started = Instant::now();
        let spec = ProcessSpec::new("sh", vec!["-c".into(), "sleep 30 & printf done".into()]);
        let output = broker(&state)
            .run_process(&spec, &CancellationToken::new())
            .unwrap();
        assert_eq!(output.stdout, b"done");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(state.status().owned_processes, 0);
    }
    #[cfg(unix)]
    #[test]
    fn revoke_kills_background_parent_and_descendants() {
        let state = DesktopProcessState::default();
        state.set_enabled(true);
        let spec = ProcessSpec::new("sh", vec!["-c".into(), "sleep 30 & wait".into()]);
        broker(&state)
            .spawn_detached(&spec, &CancellationToken::new())
            .unwrap();
        let owned = Arc::clone(&state.0.lock().unwrap().processes[0]);
        state.set_enabled(false);
        assert!(owned.lock().unwrap().child.try_wait().unwrap().is_some());
        assert_eq!(state.status().owned_processes, 0);
    }
    #[test]
    fn combined_output_is_bounded() {
        let state = DesktopProcessState::default();
        state.set_enabled(true);
        let mut spec = echo();
        spec.max_output_bytes = 2;
        let out = broker(&state)
            .run_process(&spec, &CancellationToken::new())
            .unwrap();
        assert_eq!(out.stdout.len() + out.stderr.len(), 2);
        assert!(out.truncated);
    }

    fn long_command() -> ProcessSpec {
        if cfg!(windows) {
            ProcessSpec::new("cmd", vec!["/C".into(), "ping -n 30 127.0.0.1 >NUL".into()])
        } else {
            ProcessSpec::new("sh", vec!["-c".into(), "sleep 30 & wait".into()])
        }
    }

    #[test]
    fn deadline_terminates_the_tree_and_releases_ownership() {
        let state = DesktopProcessState::default();
        state.set_enabled(true);
        let started = Instant::now();
        let spec = long_command().with_timeout(Duration::from_millis(30));
        assert!(broker(&state)
            .run_process(&spec, &CancellationToken::new())
            .is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(state.status().owned_processes, 0);
    }

    #[test]
    fn cancellation_terminates_the_tree_and_releases_ownership() {
        let state = DesktopProcessState::default();
        state.set_enabled(true);
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            signal.cancel(inbharat_harness_core::CancelCause::User);
        });
        let started = Instant::now();
        assert!(broker(&state)
            .run_process(&long_command(), &cancel)
            .is_err());
        thread.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(state.status().owned_processes, 0);
    }

    #[cfg(windows)]
    #[test]
    fn revoke_terminates_a_windows_background_job() {
        let state = DesktopProcessState::default();
        state.set_enabled(true);
        broker(&state)
            .spawn_detached(&long_command(), &CancellationToken::new())
            .unwrap();
        let owned = Arc::clone(&state.0.lock().unwrap().processes[0]);
        state.set_enabled(false);
        assert!(owned.lock().unwrap().child.try_wait().unwrap().is_some());
        assert_eq!(state.status().owned_processes, 0);
    }
}
