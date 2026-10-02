// Boot waterfall tracing (2026-10-02): a slow launch must be attributable to
// the exact step, not guessed at. Every long step of the boot chain appends
// one line — elapsed ms since first mark plus the step name — to
// %TEMP%\unoone-logs\boot-trace.log. The file is truncated at each process
// start, so the trace on disk always describes the most recent launch.
//
// Marks are fire-and-forget: a failed write is ignored (the trace is a
// diagnostic, never a dependency), and each mark costs one small append.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

static EPOCH: OnceLock<Instant> = OnceLock::new();

fn trace_path() -> std::path::PathBuf {
    std::env::temp_dir().join("unoone-logs").join("boot-trace.log")
}

/// Append a boot-step mark. The first mark in a process truncates the file so
/// stale lines from a previous launch never contaminate the waterfall.
pub fn mark(stage: &str) {
    mark_detail(stage, "");
}

/// Append a boot-step mark with a detail suffix (branch taken, counts, sizes).
pub fn mark_detail(stage: &str, detail: &str) {
    let first = EPOCH.get().is_none();
    let epoch = *EPOCH.get_or_init(Instant::now);
    let ms = epoch.elapsed().as_millis();
    let path = trace_path();
    if first {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(path.as_path()));
        let _ = std::fs::write(
            &path,
            format!(
                "# boot trace pid={} started={}\n",
                std::process::id(),
                chrono::Utc::now().to_rfc3339()
            ),
        );
    }
    let line = if detail.is_empty() {
        format!("{ms:>7}ms {stage}\n")
    } else {
        format!("{ms:>7}ms {stage} | {detail}\n")
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Mark at entry and (via Drop) at exit, so a step that hangs shows up as
/// "begin" with no matching "end" — the gap in the trace names the blocker.
pub fn step(stage: &'static str) -> StepGuard {
    mark(stage);
    StepGuard { stage }
}

pub struct StepGuard {
    stage: &'static str,
}

impl Drop for StepGuard {
    fn drop(&mut self) {
        mark(&format!("done: {}", self.stage));
    }
}