//! Stage 5 per-file diff and review model (design §5.1–5.2, owner B).
//!
//! Diffs are line-based (lines split after `\n` only, so a lone `\r` stays inside
//! its line and every byte round-trips) using `similar` Myers with a 500 ms
//! deadline. On timeout the whole file becomes ONE replace hunk (`timed_out`).
//! Rendering is bounded (5,000 lines / 256 KiB of unified text per file); beyond
//! that `truncated` is set and only whole-file decisions are allowed. Binary
//! (non-UTF-8 or NUL-containing) files are shown as "binary changed" only.
//!
//! Review decisions bind the exact base/new SHA-256 that were displayed; any
//! later change to the file makes the decision stale and [`ReviewState::reset_stale`]
//! returns it to `Pending`. Composition invariants (enforced by tests):
//! `compose_accepted(all) == current`, `compose_accepted(∅) == base`.
//! Nothing here sets `Accepted` on its own: decisions come only from a
//! `UiReviewEvent` the orchestrator (owner A) accepts from the main window.

use crate::isolation::hash;
use crate::task_workspace::{is_text, validate_path, WorkingSet, HARD_MAX_FILE_BYTES};
use serde::{Deserialize, Serialize};
use similar::{Algorithm, ChangeTag, TextDiff};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

pub const DIFF_CONTEXT: usize = 3;
pub const DIFF_TIMEOUT: Duration = Duration::from_millis(500);
pub const MAX_RENDERED_LINES: usize = 5_000;
pub const MAX_UNIFIED_BYTES: usize = 256 * 1024;
/// Reserved for the two file header lines (paths are ≤256 bytes).
const HEADER_RESERVE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Added,
    Modified,
    Deleted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineTag {
    Context,
    Delete,
    Insert,
}
/// One diff line. `text` is the EXACT line including its terminator (`\n`,
/// `\r\n`); only a file's final line may lack one ("No newline at end of file").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffLine {
    pub tag: LineTag,
    pub text: String,
}
/// Unified-diff numbering: 1-based start, except a zero-length side whose start
/// is the number of lines before the position (GNU/difflib convention).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hunk {
    pub index: u32,
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    pub lines: Vec<DiffLine>,
    pub hunk_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDiff {
    pub path: String,
    pub change: Change,
    pub base_sha256: Option<String>,
    pub new_sha256: Option<String>,
    pub binary: bool,
    /// Full hunks (needed for export even when the display is truncated). The UI
    /// renders `unified`; hunk controls only when `hunk_controls_enabled()`.
    pub hunks: Vec<Hunk>,
    /// Display text, ≤256 KiB and ≤5,000 lines.
    pub unified: String,
    pub truncated: bool,
    pub timed_out: bool,
}
impl FileDiff {
    pub fn hunk_controls_enabled(&self) -> bool {
        !self.binary && !self.truncated && !self.timed_out && self.change == Change::Modified
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffError {
    Binary,
    TimedOut,
    Truncated,
    UnknownHunk(u32),
}
impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "diff: {self:?}")
    }
}
impl std::error::Error for DiffError {}

#[derive(Serialize)]
struct HunkDigest<'a> {
    index: u32,
    old_start: u32,
    old_len: u32,
    new_start: u32,
    new_len: u32,
    lines: &'a [DiffLine],
}
fn hunk_digest(h: &Hunk) -> String {
    let d = HunkDigest {
        index: h.index,
        old_start: h.old_start,
        old_len: h.old_len,
        new_start: h.new_start,
        new_len: h.new_len,
        lines: &h.lines,
    };
    hash(&serde_json::to_vec(&d).unwrap_or_default())
}
fn n32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
fn start_of(pos: usize, len: usize) -> u32 {
    n32(if len == 0 { pos } else { pos + 1 })
}
/// Line index where a hunk side begins.
fn pos_of(start: u32, len: u32) -> usize {
    if len == 0 {
        start as usize
    } else {
        (start as usize).saturating_sub(1)
    }
}
fn make_hunk(index: usize, old: (usize, usize), new: (usize, usize), lines: Vec<DiffLine>) -> Hunk {
    let mut h = Hunk {
        index: n32(index),
        old_start: start_of(old.0, old.1),
        old_len: n32(old.1),
        new_start: start_of(new.0, new.1),
        new_len: n32(new.1),
        lines,
        hunk_sha256: String::new(),
    };
    h.hunk_sha256 = hunk_digest(&h);
    h
}

struct Computed {
    hunks: Vec<Hunk>,
    timed_out: bool,
}

fn compute(old: &str, new: &str, timeout: Duration) -> Computed {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    if old_lines == new_lines {
        return Computed {
            hunks: Vec::new(),
            timed_out: false,
        };
    }
    let deadline = Instant::now().checked_add(timeout);
    let mut config = TextDiff::configure();
    config.algorithm(Algorithm::Myers);
    if let Some(d) = deadline {
        config.deadline(d);
    }
    let diff = config.diff_slices(&old_lines, &new_lines);
    // Myers gives up silently at the deadline; treat reaching it as a timeout.
    if deadline.is_some_and(|d| Instant::now() >= d) {
        let mut lines: Vec<DiffLine> = old_lines
            .iter()
            .map(|t| DiffLine {
                tag: LineTag::Delete,
                text: (*t).to_owned(),
            })
            .collect();
        lines.extend(new_lines.iter().map(|t| DiffLine {
            tag: LineTag::Insert,
            text: (*t).to_owned(),
        }));
        return Computed {
            hunks: vec![make_hunk(
                0,
                (0, old_lines.len()),
                (0, new_lines.len()),
                lines,
            )],
            timed_out: true,
        };
    }
    let mut hunks = Vec::new();
    for (index, group) in diff.grouped_ops(DIFF_CONTEXT).iter().enumerate() {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_range = (
            first.old_range().start,
            last.old_range().end - first.old_range().start,
        );
        let new_range = (
            first.new_range().start,
            last.new_range().end - first.new_range().start,
        );
        let mut lines = Vec::new();
        for op in group {
            for change in diff.iter_changes(op) {
                lines.push(DiffLine {
                    tag: match change.tag() {
                        ChangeTag::Equal => LineTag::Context,
                        ChangeTag::Delete => LineTag::Delete,
                        ChangeTag::Insert => LineTag::Insert,
                    },
                    text: change.value().to_owned(),
                });
            }
        }
        hunks.push(make_hunk(index, old_range, new_range, lines));
    }
    Computed {
        hunks,
        timed_out: false,
    }
}

fn range(start: u32, len: u32) -> String {
    if len == 1 {
        start.to_string()
    } else {
        format!("{start},{len}")
    }
}
fn render_hunk(out: &mut String, h: &Hunk, new_start: u32) {
    let _ = writeln!(
        out,
        "@@ -{} +{} @@",
        range(h.old_start, h.old_len),
        range(new_start, h.new_len)
    );
    for line in &h.lines {
        out.push(match line.tag {
            LineTag::Context => ' ',
            LineTag::Delete => '-',
            LineTag::Insert => '+',
        });
        out.push_str(&line.text);
        if !line.text.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
}
fn names(path: &str, change: Change) -> (String, String) {
    (
        if change == Change::Added {
            "/dev/null".into()
        } else {
            format!("a/{path}")
        },
        if change == Change::Deleted {
            "/dev/null".into()
        } else {
            format!("b/{path}")
        },
    )
}
/// Git's convention: a name containing a space is followed by a TAB, which
/// GNU patch (and our strict applier) treat as the end of the file name.
fn file_header(path: &str, change: Change) -> String {
    let (a, b) = names(path, change);
    let tab = |n: &str| if n.contains(' ') { "\t" } else { "" };
    format!("--- {a}{}\n+++ {b}{}\n", tab(&a), tab(&b))
}
fn render_body(hunks: &[Hunk]) -> String {
    let mut body = String::new();
    for h in hunks {
        render_hunk(&mut body, h, h.new_start);
    }
    body
}
/// Path-independent (so `compose_accepted` decides identically): the body plus
/// the two header lines must fit 5,000 lines and 256 KiB.
fn too_big(body: &str) -> bool {
    body.len() + HEADER_RESERVE > MAX_UNIFIED_BYTES || body.lines().count() + 2 > MAX_RENDERED_LINES
}

/// Diff of one path. `None` = absent on that side.
pub fn diff_file(path: &str, base: Option<&[u8]>, current: Option<&[u8]>) -> FileDiff {
    diff_file_with(path, base, current, DIFF_TIMEOUT)
}
fn diff_file_with(
    path: &str,
    base: Option<&[u8]>,
    current: Option<&[u8]>,
    timeout: Duration,
) -> FileDiff {
    let change = match (base, current) {
        (None, Some(_)) => Change::Added,
        (Some(_), None) => Change::Deleted,
        _ => Change::Modified,
    };
    let mut diff = FileDiff {
        path: path.to_owned(),
        change,
        base_sha256: base.map(hash),
        new_sha256: current.map(hash),
        binary: false,
        hunks: Vec::new(),
        unified: String::new(),
        truncated: false,
        timed_out: false,
    };
    let (old, new) = (base.unwrap_or_default(), current.unwrap_or_default());
    let (Some(old), Some(new)) = (
        is_text(old)
            .then(|| std::str::from_utf8(old).ok())
            .flatten(),
        is_text(new)
            .then(|| std::str::from_utf8(new).ok())
            .flatten(),
    ) else {
        diff.binary = true;
        let (a, b) = names(path, change);
        diff.unified = format!("Binary files {a} and {b} differ\n");
        return diff;
    };
    let computed = compute(old, new, timeout);
    let body = render_body(&computed.hunks);
    diff.timed_out = computed.timed_out;
    diff.unified = file_header(path, change);
    if too_big(&body) {
        diff.truncated = true;
        let budget = MAX_UNIFIED_BYTES - HEADER_RESERVE;
        let mut used = 0;
        for line in body.split_inclusive('\n').take(MAX_RENDERED_LINES - 2) {
            if used + line.len() > budget {
                break;
            }
            used += line.len();
        }
        diff.unified.push_str(&body[..used]);
    } else {
        diff.unified.push_str(&body);
    }
    diff.hunks = computed.hunks;
    diff
}

/// Base → current for every changed path, sorted by path, 3 lines of context.
pub fn diff_working_set(m: &WorkingSet) -> Vec<FileDiff> {
    m.changed_paths()
        .iter()
        .map(|p| {
            diff_file(
                p,
                m.base_files().get(p).map(Vec::as_slice),
                m.files().get(p).map(Vec::as_slice),
            )
        })
        .collect()
}

/// Base with exactly the accepted hunks of base→current applied. `∅` → base.
/// Partial selections are refused (never guessed) when the diff is binary,
/// timed out or truncated, or an index is unknown.
pub fn compose_accepted(
    base: &[u8],
    current: &[u8],
    accepted_hunks: &BTreeSet<u32>,
) -> Result<Vec<u8>, DiffError> {
    compose_with(base, current, accepted_hunks, DIFF_TIMEOUT)
}
fn compose_with(
    base: &[u8],
    current: &[u8],
    accepted: &BTreeSet<u32>,
    timeout: Duration,
) -> Result<Vec<u8>, DiffError> {
    if accepted.is_empty() {
        return Ok(base.to_vec());
    }
    let (Some(old), Some(new)) = (
        is_text(base)
            .then(|| std::str::from_utf8(base).ok())
            .flatten(),
        is_text(current)
            .then(|| std::str::from_utf8(current).ok())
            .flatten(),
    ) else {
        return Err(DiffError::Binary);
    };
    let computed = compute(old, new, timeout);
    if computed.timed_out {
        return Err(DiffError::TimedOut);
    }
    if too_big(&render_body(&computed.hunks)) {
        return Err(DiffError::Truncated);
    }
    if let Some(bad) = accepted
        .iter()
        .find(|i| **i as usize >= computed.hunks.len())
    {
        return Err(DiffError::UnknownHunk(*bad));
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let mut out = String::with_capacity(old.len().max(new.len()));
    let mut cursor = 0;
    for h in &computed.hunks {
        let at = pos_of(h.old_start, h.old_len);
        old_lines[cursor..at].iter().for_each(|l| out.push_str(l));
        let keep = if accepted.contains(&h.index) {
            LineTag::Delete
        } else {
            LineTag::Insert
        };
        h.lines
            .iter()
            .filter(|l| l.tag != keep)
            .for_each(|l| out.push_str(&l.text));
        cursor = at + h.old_len as usize;
    }
    old_lines[cursor..].iter().for_each(|l| out.push_str(l));
    Ok(out.into_bytes())
}

// ---- review model (§5.2): owner B types; owner A enforces in the orchestrator ----

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewState {
    pub files: BTreeMap<String, FileReview>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileReview {
    pub decision: FileDecision,
    pub reviewed_base_sha256: Option<String>,
    pub reviewed_new_sha256: Option<String>,
    pub ui_event_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileDecision {
    Pending,
    Accepted,
    Rejected,
    PartiallyAccepted {
        hunks: BTreeSet<u32>,
        composed_sha256: String,
    },
}
/// Sent by the main-window UI only. Carries the hashes the user SAW.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiReviewEvent {
    pub task_id: String,
    pub view_seq: u64,
    pub path: String,
    pub decision: FileDecision,
    pub displayed_base_sha256: Option<String>,
    pub displayed_new_sha256: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewError {
    InvalidPath,
    NotChanged,
    /// Displayed or bound hashes differ from the current content.
    Stale {
        path: String,
    },
    Undecided {
        path: String,
    },
    NothingAccepted,
    /// Hunk selection not allowed (added/deleted/binary/truncated/timed-out file,
    /// or an empty selection): decide the whole file.
    WholeFileOnly,
    TooLarge,
    Diff(DiffError),
}
impl fmt::Display for ReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "review: {self:?}")
    }
}
impl std::error::Error for ReviewError {}

fn side_hashes(ws: &WorkingSet, path: &str) -> (Option<String>, Option<String>) {
    (
        ws.base_files().get(path).map(|b| hash(b)),
        ws.files().get(path).map(|b| hash(b)),
    )
}

/// Validates one UI decision against the CURRENT working set and returns the
/// review record that binds the displayed hashes. For a partial acceptance the
/// composed content is recomputed on the server; a non-empty event
/// `composed_sha256` must match it.
pub fn decide(
    ws: &WorkingSet,
    ev: &UiReviewEvent,
    ui_event_id: &str,
) -> Result<FileReview, ReviewError> {
    validate_path(&ev.path).map_err(|_| ReviewError::InvalidPath)?;
    let base = ws.base_files().get(&ev.path);
    let current = ws.files().get(&ev.path);
    if base == current {
        return Err(ReviewError::NotChanged);
    }
    let (bh, ch) = side_hashes(ws, &ev.path);
    if ev.displayed_base_sha256 != bh || ev.displayed_new_sha256 != ch {
        return Err(ReviewError::Stale {
            path: ev.path.clone(),
        });
    }
    let decision = match &ev.decision {
        FileDecision::PartiallyAccepted {
            hunks,
            composed_sha256,
        } => {
            let (Some(b), Some(c)) = (base, current) else {
                return Err(ReviewError::WholeFileOnly);
            };
            if hunks.is_empty() {
                return Err(ReviewError::WholeFileOnly);
            }
            let composed = compose_accepted(b, c, hunks).map_err(|e| match e {
                DiffError::UnknownHunk(_) => ReviewError::Diff(e),
                _ => ReviewError::WholeFileOnly,
            })?;
            if composed.len() > HARD_MAX_FILE_BYTES {
                return Err(ReviewError::TooLarge);
            }
            let sha = hash(&composed);
            if !composed_sha256.is_empty() && *composed_sha256 != sha {
                return Err(ReviewError::Stale {
                    path: ev.path.clone(),
                });
            }
            FileDecision::PartiallyAccepted {
                hunks: hunks.clone(),
                composed_sha256: sha,
            }
        }
        other => other.clone(),
    };
    Ok(FileReview {
        decision,
        reviewed_base_sha256: bh,
        reviewed_new_sha256: ch,
        ui_event_id: ui_event_id.to_owned(),
    })
}

/// The coherent post-image to write into the task worktree: every base ∪
/// current path → accepted content (`None` = absent). Bytes are never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct ApplyPlan {
    pub files: BTreeMap<String, Option<Vec<u8>>>,
    pub post_image: BTreeMap<String, Option<String>>,
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
}
impl fmt::Debug for ApplyPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplyPlan")
            .field("post_image", &self.post_image)
            .field("accepted", &self.accepted)
            .field("rejected", &self.rejected)
            .finish()
    }
}
impl ApplyPlan {
    /// SHA-256 over the post-image manifest and the per-path decisions (one
    /// candidate for the `displayed_change_set_sha256` echoed in `UiApplyEvent`).
    pub fn change_set_sha256(&self) -> String {
        hash(
            &serde_json::to_vec(&(&self.post_image, &self.accepted, &self.rejected))
                .unwrap_or_default(),
        )
    }
}

impl ReviewState {
    /// Validate (see [`decide`]) and store one UI decision.
    pub fn record(
        &mut self,
        ws: &WorkingSet,
        ev: &UiReviewEvent,
        ui_event_id: &str,
    ) -> Result<(), ReviewError> {
        let review = decide(ws, ev, ui_event_id)?;
        self.files.insert(ev.path.clone(), review);
        Ok(())
    }
    /// Non-pending decisions whose bound hashes no longer equal the content.
    pub fn stale_paths(&self, ws: &WorkingSet) -> Vec<String> {
        self.files
            .iter()
            .filter(|(path, r)| {
                r.decision != FileDecision::Pending
                    && side_hashes(ws, path)
                        != (
                            r.reviewed_base_sha256.clone(),
                            r.reviewed_new_sha256.clone(),
                        )
            })
            .map(|(p, _)| p.clone())
            .collect()
    }
    /// After any edit, repair or copy-out: stale decisions go back to `Pending`.
    /// Returns the reset paths (each is a `StaleReview` risk).
    pub fn reset_stale(&mut self, ws: &WorkingSet) -> Vec<String> {
        let stale = self.stale_paths(ws);
        for path in &stale {
            self.files.insert(
                path.clone(),
                FileReview {
                    decision: FileDecision::Pending,
                    reviewed_base_sha256: None,
                    reviewed_new_sha256: None,
                    ui_event_id: String::new(),
                },
            );
        }
        stale
    }
    /// After `WorkingSet::revert_file` (LedgerOnly): the decision is `Rejected`,
    /// bound to the restored content.
    pub fn mark_reverted(&mut self, ws: &WorkingSet, path: &str, ui_event_id: &str) {
        let (bh, ch) = side_hashes(ws, path);
        self.files.insert(
            path.to_owned(),
            FileReview {
                decision: FileDecision::Rejected,
                reviewed_base_sha256: bh,
                reviewed_new_sha256: ch,
                ui_event_id: ui_event_id.to_owned(),
            },
        );
    }
    /// Changed files without a current, non-pending decision.
    pub fn undecided(&self, ws: &WorkingSet) -> Vec<String> {
        ws.changed_paths()
            .into_iter()
            .filter(|p| match self.files.get(p) {
                None => true,
                Some(r) => {
                    r.decision == FileDecision::Pending
                        || side_hashes(ws, p)
                            != (
                                r.reviewed_base_sha256.clone(),
                                r.reviewed_new_sha256.clone(),
                            )
                }
            })
            .collect()
    }
    /// Apply preconditions (design §5.2): every changed file decided on its
    /// current hashes, at least one accepted, partial compositions re-verified.
    pub fn apply_plan(&self, ws: &WorkingSet) -> Result<ApplyPlan, ReviewError> {
        let mut plan = ApplyPlan {
            files: BTreeMap::new(),
            post_image: BTreeMap::new(),
            accepted: Vec::new(),
            rejected: Vec::new(),
        };
        let paths: BTreeSet<&String> = ws.base_files().keys().chain(ws.files().keys()).collect();
        for path in paths {
            let base = ws.base_files().get(path);
            let current = ws.files().get(path);
            if base == current {
                plan.files.insert(path.clone(), base.cloned());
                continue;
            }
            let undecided = || ReviewError::Undecided { path: path.clone() };
            let review = self.files.get(path).ok_or_else(undecided)?;
            if review.decision == FileDecision::Pending {
                return Err(undecided());
            }
            if side_hashes(ws, path)
                != (
                    review.reviewed_base_sha256.clone(),
                    review.reviewed_new_sha256.clone(),
                )
            {
                return Err(ReviewError::Stale { path: path.clone() });
            }
            let content = match &review.decision {
                FileDecision::Pending => return Err(undecided()),
                FileDecision::Accepted => {
                    plan.accepted.push(path.clone());
                    current.cloned()
                }
                FileDecision::Rejected => {
                    plan.rejected.push(path.clone());
                    base.cloned()
                }
                FileDecision::PartiallyAccepted {
                    hunks,
                    composed_sha256,
                } => {
                    let (Some(b), Some(c)) = (base, current) else {
                        return Err(ReviewError::WholeFileOnly);
                    };
                    let composed = compose_accepted(b, c, hunks).map_err(ReviewError::Diff)?;
                    if hash(&composed) != *composed_sha256 {
                        return Err(ReviewError::Stale { path: path.clone() });
                    }
                    plan.accepted.push(path.clone());
                    Some(composed)
                }
            };
            plan.files.insert(path.clone(), content);
        }
        if plan.accepted.is_empty() {
            return Err(ReviewError::NothingAccepted);
        }
        plan.post_image = plan
            .files
            .iter()
            .map(|(p, b)| (p.clone(), b.as_ref().map(|b| hash(b))))
            .collect();
        Ok(plan)
    }
}

/// Unified patch (`a/` `b/` headers, `/dev/null` for added/deleted) of ONLY the
/// accepted content: `Accepted` files in full, `PartiallyAccepted` files with
/// just their selected hunks (new-side numbers recomputed). Pending, rejected,
/// stale (bound hashes ≠ diff hashes), tampered (hunk digest/index mismatch) and
/// invalid-path entries are omitted. Binary files appear only as the standard
/// "Binary files … differ" notice. For the user's own tools; Stage 5 never
/// writes the user's repository.
pub fn export_patch(diffs: &[FileDiff], accepted: &ReviewState) -> String {
    let mut sorted: Vec<&FileDiff> = diffs.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let duplicated: BTreeSet<&str> = sorted
        .windows(2)
        .filter(|w| w[0].path == w[1].path)
        .map(|w| w[0].path.as_str())
        .collect();
    let mut out = String::new();
    for d in sorted {
        if duplicated.contains(d.path.as_str()) || validate_path(&d.path).is_err() {
            continue;
        }
        let Some(review) = accepted.files.get(&d.path) else {
            continue;
        };
        if review.reviewed_base_sha256 != d.base_sha256
            || review.reviewed_new_sha256 != d.new_sha256
        {
            continue;
        }
        let selected: Vec<&Hunk> = match &review.decision {
            FileDecision::Accepted => d.hunks.iter().collect(),
            FileDecision::PartiallyAccepted { hunks, .. } => {
                if !d.hunk_controls_enabled()
                    || hunks.is_empty()
                    || hunks.iter().any(|i| *i as usize >= d.hunks.len())
                {
                    continue;
                }
                d.hunks
                    .iter()
                    .filter(|h| hunks.contains(&h.index))
                    .collect()
            }
            FileDecision::Pending | FileDecision::Rejected => continue,
        };
        if d.binary {
            let (a, b) = names(&d.path, d.change);
            let _ = writeln!(out, "Binary files {a} and {b} differ");
            continue;
        }
        if d.hunks
            .iter()
            .enumerate()
            .any(|(i, h)| h.index as usize != i || hunk_digest(h) != h.hunk_sha256)
        {
            continue;
        }
        out.push_str(&file_header(&d.path, d.change));
        let mut delta: i64 = 0;
        for h in selected {
            let at = pos_of(h.old_start, h.old_len) as i64 + delta;
            let new_start = if h.new_len == 0 { at } else { at + 1 };
            render_hunk(&mut out, h, u32::try_from(new_start).unwrap_or(0));
            delta += i64::from(h.new_len) - i64::from(h.old_len);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_workspace::{PathPolicy, ProposedEdit};

    /// Deterministic generator (no external crates): xorshift64*.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }
    fn text_case(rng: &mut Rng) -> (String, String) {
        let vocab = [
            "a",
            "b",
            "c",
            "def f():",
            "    return 1",
            "",
            "x = 1",
            "# note",
            "\tpass",
            "b",
        ];
        let eol = if rng.below(5) == 0 { "\r\n" } else { "\n" };
        let n = rng.below(40) as usize;
        let mut base: Vec<String> = (0..n)
            .map(|_| format!("{}{eol}", vocab[rng.below(vocab.len() as u64) as usize]))
            .collect();
        if !base.is_empty() && rng.below(4) == 0 {
            let last = base.pop().unwrap();
            base.push(last.trim_end_matches(['\r', '\n']).to_owned());
        }
        let mut cur = base.clone();
        for _ in 0..rng.below(8) {
            let at = rng.below(cur.len() as u64 + 1) as usize;
            match rng.below(3) {
                0 if at < cur.len() => {
                    cur.remove(at);
                }
                1 if at < cur.len() => cur[at] = format!("changed {}{eol}", rng.below(1000)),
                _ => cur.insert(at, format!("inserted {}{eol}", rng.below(1000))),
            }
        }
        // Only the final line may lack a terminator.
        let fix = |v: &mut Vec<String>| {
            let n = v.len();
            for (i, l) in v.iter_mut().enumerate() {
                if i + 1 < n && !l.ends_with('\n') {
                    l.push_str(eol);
                }
            }
        };
        fix(&mut cur);
        (base.concat(), cur.concat())
    }
    fn all(d: &FileDiff) -> BTreeSet<u32> {
        d.hunks.iter().map(|h| h.index).collect()
    }
    fn changes(d: &FileDiff) -> usize {
        d.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.tag != LineTag::Context)
            .count()
    }
    /// Our own strict applier (task_workspace) must accept our own rendering.
    fn reapply(base: &str, unified: &str) -> String {
        let f: BTreeMap<String, Vec<u8>> = [("f.py".to_owned(), base.as_bytes().to_vec())].into();
        let mut ws = WorkingSet::from_parts(f.clone(), f).unwrap();
        let policy = PathPolicy::new(["f.py".to_owned()].into(), BTreeSet::new(), vec![]);
        if let Err(e) = ws.apply_edit(
            &policy,
            &ProposedEdit::Patch {
                path: "f.py".into(),
                unified_hunks: unified.into(),
            },
        ) {
            panic!("reapply {e:?}\nBASE={base:?}\nUNIFIED={unified:?}");
        }
        String::from_utf8(ws.files()["f.py"].clone()).unwrap()
    }

    #[test]
    fn task_diff_compose_properties_all_none_subset() {
        let mut rng = Rng(0x5eed_5a5e_0000_0005);
        let (mut cases, mut multi) = (0, 0);
        for _ in 0..400 {
            let (base, cur) = text_case(&mut rng);
            let (b, c) = (base.as_bytes(), cur.as_bytes());
            let d = diff_file("f.py", Some(b), Some(c));
            assert!(!d.binary && !d.truncated && !d.timed_out);
            assert_eq!(
                compose_accepted(b, c, &all(&d)).unwrap(),
                c,
                "all = current"
            );
            assert_eq!(
                compose_accepted(b, c, &BTreeSet::new()).unwrap(),
                b,
                "none = base"
            );
            if base == cur {
                assert!(d.hunks.is_empty());
                continue;
            }
            cases += 1;
            assert_eq!(reapply(&base, &d.unified), cur, "rendered patch re-applies");
            for (i, h) in d.hunks.iter().enumerate() {
                assert_eq!(h.index as usize, i);
                assert_eq!(hunk_digest(h), h.hunk_sha256);
            }
            if d.hunks.len() < 2 {
                continue;
            }
            multi += 1;
            let subset: BTreeSet<u32> = all(&d).into_iter().filter(|_| rng.below(2) == 0).collect();
            let x = compose_accepted(b, c, &subset).unwrap();
            let xs = std::str::from_utf8(&x).unwrap();
            // Round trips: base→X accepts back to X, X→current accepts to current.
            let bx = diff_file("f.py", Some(b), Some(&x));
            assert_eq!(compose_accepted(b, &x, &all(&bx)).unwrap(), x);
            let xc = diff_file("f.py", Some(&x), Some(c));
            assert_eq!(compose_accepted(&x, c, &all(&xc)).unwrap(), c);
            if !xc.hunks.is_empty() {
                assert_eq!(reapply(xs, &xc.unified), cur);
            }
            let chosen: usize = d
                .hunks
                .iter()
                .filter(|h| subset.contains(&h.index))
                .flat_map(|h| &h.lines)
                .filter(|l| l.tag != LineTag::Context)
                .count();
            assert!(
                changes(&bx) <= chosen,
                "base→X needs no more than the chosen hunks"
            );
            assert!(
                changes(&xc) <= changes(&d) - chosen,
                "X→current needs no more than the rest"
            );
            // Exported partial patch == X when applied to base.
            let mut review = ReviewState::default();
            review.files.insert(
                "f.py".into(),
                FileReview {
                    decision: FileDecision::PartiallyAccepted {
                        hunks: subset.clone(),
                        composed_sha256: hash(&x),
                    },
                    reviewed_base_sha256: d.base_sha256.clone(),
                    reviewed_new_sha256: d.new_sha256.clone(),
                    ui_event_id: "synthetic".into(),
                },
            );
            let exported = export_patch(std::slice::from_ref(&d), &review);
            if subset.is_empty() {
                assert!(exported.is_empty());
            } else {
                assert_eq!(reapply(&base, &exported), xs);
            }
        }
        println!("STAGE5_B_COMPOSE cases={cases} multi_hunk={multi}");
        assert!(cases > 300 && multi > 100, "property space exercised");
        // Unknown hunk / binary.
        assert_eq!(
            compose_accepted(b"a\n", b"b\n", &[7].into()),
            Err(DiffError::UnknownHunk(7))
        );
        assert_eq!(
            compose_accepted(b"a\n", b"a\n", &[0].into()),
            Err(DiffError::UnknownHunk(0))
        );
        assert_eq!(
            compose_accepted(b"\x00", b"\x01", &[0].into()),
            Err(DiffError::Binary)
        );
        assert_eq!(
            compose_accepted(b"\x00", b"\x01", &BTreeSet::new()).unwrap(),
            b"\x00"
        );
    }

    #[test]
    fn task_diff_binary_truncated_and_timeout() {
        // Binary: shown as changed, never hunk-applied.
        let d = diff_file("x.bin", Some(b"\x00\x01"), Some(b"\x00\x02"));
        assert!(d.binary && d.hunks.is_empty() && !d.hunk_controls_enabled());
        assert_eq!(d.unified, "Binary files a/x.bin and b/x.bin differ\n");
        let d = diff_file("x.bin", None, Some(b"\xff\xfe"));
        assert_eq!(
            (d.change, d.unified.as_str()),
            (Change::Added, "Binary files /dev/null and b/x.bin differ\n")
        );
        let d = diff_file("x.txt", Some(b"ok\n"), Some(b"bad\xffutf8\n"));
        assert!(d.binary);
        // Truncated by line count: hunks stay complete, display is bounded,
        // hunk controls are off, partial composition refused, export still exact.
        // (Every 5th line changed keeps Myers well inside its deadline; a file
        // with EVERY line changed legitimately hits the 500 ms timeout instead.)
        let base: String = (0..6000).map(|i| format!("line {i}\n")).collect();
        let cur: String = (0..6000)
            .map(|i| {
                if i % 5 == 0 {
                    format!("LINE {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        let d = diff_file("big.py", Some(base.as_bytes()), Some(cur.as_bytes()));
        // The Myers deadline is wall-clock (500 ms). On a loaded host (debug
        // build, parallel tests) this file can hit the deadline instead of the
        // line-count bound. Both outcomes are fail-closed and must disable hunk
        // controls and refuse partial composition; only the error kind varies.
        assert!((d.truncated || d.timed_out) && !d.hunk_controls_enabled());
        assert!(
            d.unified.len() <= MAX_UNIFIED_BYTES && d.unified.lines().count() <= MAX_RENDERED_LINES
        );
        assert!(matches!(
            compose_accepted(base.as_bytes(), cur.as_bytes(), &[0].into()),
            Err(DiffError::Truncated) | Err(DiffError::TimedOut)
        ));
        assert_eq!(
            compose_accepted(base.as_bytes(), cur.as_bytes(), &BTreeSet::new()).unwrap(),
            base.as_bytes()
        );
        let review = |d: &FileDiff, decision: FileDecision| ReviewState {
            files: [(
                d.path.clone(),
                FileReview {
                    decision,
                    reviewed_base_sha256: d.base_sha256.clone(),
                    reviewed_new_sha256: d.new_sha256.clone(),
                    ui_event_id: "synthetic".into(),
                },
            )]
            .into(),
        };
        let exported = export_patch(
            std::slice::from_ref(&d),
            &review(&d, FileDecision::Accepted),
        );
        assert_eq!(reapply(&base, &exported.replace("big.py", "f.py")), cur);
        let partial = FileDecision::PartiallyAccepted {
            hunks: [0].into(),
            composed_sha256: String::new(),
        };
        assert!(export_patch(std::slice::from_ref(&d), &review(&d, partial)).is_empty());
        // Truncated by bytes: few, long lines.
        let base: String = (0..100)
            .map(|i| format!("{i} {}\n", "a".repeat(3000)))
            .collect();
        let cur: String = (0..100)
            .map(|i| format!("{i} {}\n", "b".repeat(3000)))
            .collect();
        let d = diff_file("wide.py", Some(base.as_bytes()), Some(cur.as_bytes()));
        assert!(d.truncated && d.unified.len() <= MAX_UNIFIED_BYTES);
        // Timeout: forced zero deadline → one replace hunk, flagged.
        let (b, c) = ("a\nb\nc\nd\n", "a\nB\nc\nD\n");
        let d = diff_file_with(
            "t.py",
            Some(b.as_bytes()),
            Some(c.as_bytes()),
            Duration::ZERO,
        );
        assert!(d.timed_out && !d.hunk_controls_enabled());
        assert_eq!(d.hunks.len(), 1);
        let h = &d.hunks[0];
        assert_eq!(
            (h.old_start, h.old_len, h.new_start, h.new_len),
            (1, 4, 1, 4)
        );
        assert!(h.lines[..4].iter().all(|l| l.tag == LineTag::Delete));
        assert!(h.lines[4..].iter().all(|l| l.tag == LineTag::Insert));
        assert_eq!(reapply(b, &d.unified.replace("t.py", "f.py")), c);
        assert_eq!(
            compose_with(b.as_bytes(), c.as_bytes(), &[0].into(), Duration::ZERO),
            Err(DiffError::TimedOut)
        );
        let exported = export_patch(
            std::slice::from_ref(&d),
            &review(&d, FileDecision::Accepted),
        );
        assert_eq!(reapply(b, &exported.replace("t.py", "f.py")), c);
        // The same input with the real deadline is a normal 2-hunk-capable diff.
        let normal = diff_file("t.py", Some(b.as_bytes()), Some(c.as_bytes()));
        assert!(!normal.timed_out && normal.hunk_controls_enabled());
    }

    fn event(d: &FileDiff, decision: FileDecision) -> UiReviewEvent {
        UiReviewEvent {
            task_id: "0".repeat(32),
            view_seq: 1,
            path: d.path.clone(),
            decision,
            displayed_base_sha256: d.base_sha256.clone(),
            displayed_new_sha256: d.new_sha256.clone(),
        }
    }

    #[test]
    fn task_diff_stale_review_resets_on_change() {
        let base: BTreeMap<String, Vec<u8>> = [
            (
                "a.py".to_owned(),
                b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n".to_vec(),
            ),
            ("b.py".to_owned(), b"b\n".to_vec()),
        ]
        .into();
        let mut ws = WorkingSet::from_parts(base.clone(), base.clone()).unwrap();
        let policy = PathPolicy::new(
            base.keys().cloned().collect(),
            BTreeSet::new(),
            vec!["new/".into()],
        );
        let edit = |ws: &mut WorkingSet, p: &str, c: &str| {
            ws.apply_edit(
                &policy,
                &ProposedEdit::Replace {
                    path: p.into(),
                    content: c.into(),
                },
            )
            .unwrap()
        };
        edit(
            &mut ws,
            "a.py",
            "ONE\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\nTWELVE\n",
        );
        let d = diff_working_set(&ws);
        assert_eq!(d.len(), 1);
        let a = &d[0];
        assert_eq!(a.hunks.len(), 2);
        let mut review = ReviewState::default();
        // Unchanged file / wrong displayed hash / invalid path.
        let mut unchanged = event(a, FileDecision::Accepted);
        unchanged.path = "b.py".into();
        assert_eq!(
            review.record(&ws, &unchanged, "e0"),
            Err(ReviewError::NotChanged)
        );
        let mut wrong = event(a, FileDecision::Accepted);
        wrong.displayed_new_sha256 = Some(hash(b"something the user did not see"));
        assert_eq!(
            review.record(&ws, &wrong, "e1"),
            Err(ReviewError::Stale {
                path: "a.py".into()
            })
        );
        let mut bad = event(a, FileDecision::Accepted);
        bad.path = "../a.py".into();
        assert_eq!(
            review.record(&ws, &bad, "e1b"),
            Err(ReviewError::InvalidPath)
        );
        assert_eq!(
            review.apply_plan(&ws).unwrap_err(),
            ReviewError::Undecided {
                path: "a.py".into()
            }
        );
        // Partial acceptance: server-composed hash, wrong client hash refused.
        let bad_partial = FileDecision::PartiallyAccepted {
            hunks: [1].into(),
            composed_sha256: hash(b"x"),
        };
        assert_eq!(
            review.record(&ws, &event(a, bad_partial), "e2"),
            Err(ReviewError::Stale {
                path: "a.py".into()
            })
        );
        let empty = FileDecision::PartiallyAccepted {
            hunks: BTreeSet::new(),
            composed_sha256: String::new(),
        };
        assert_eq!(
            review.record(&ws, &event(a, empty), "e3"),
            Err(ReviewError::WholeFileOnly)
        );
        let unknown = FileDecision::PartiallyAccepted {
            hunks: [9].into(),
            composed_sha256: String::new(),
        };
        assert_eq!(
            review.record(&ws, &event(a, unknown), "e3b"),
            Err(ReviewError::Diff(DiffError::UnknownHunk(9)))
        );
        let partial = FileDecision::PartiallyAccepted {
            hunks: [1].into(),
            composed_sha256: String::new(),
        };
        review.record(&ws, &event(a, partial), "e4").unwrap();
        let plan = review.apply_plan(&ws).unwrap();
        assert_eq!(
            plan.files["a.py"].as_deref(),
            Some(b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\nTWELVE\n".as_slice())
        );
        assert_eq!(plan.accepted, vec!["a.py"]);
        // Any later change makes the decision stale; reset → Pending.
        edit(
            &mut ws,
            "a.py",
            "ONE\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\nTWELVE!\n",
        );
        assert_eq!(review.stale_paths(&ws), vec!["a.py"]);
        assert_eq!(
            review.apply_plan(&ws).unwrap_err(),
            ReviewError::Stale {
                path: "a.py".into()
            }
        );
        assert_eq!(review.reset_stale(&ws), vec!["a.py"]);
        assert_eq!(review.files["a.py"].decision, FileDecision::Pending);
        assert_eq!(review.undecided(&ws), vec!["a.py"]);
        assert_eq!(
            review.apply_plan(&ws).unwrap_err(),
            ReviewError::Undecided {
                path: "a.py".into()
            }
        );
        // Re-review the NEW content; copy-out style change also resets.
        let d = diff_working_set(&ws);
        review
            .record(&ws, &event(&d[0], FileDecision::Accepted), "e5")
            .unwrap();
        assert!(review.stale_paths(&ws).is_empty() && review.undecided(&ws).is_empty());
        edit(&mut ws, "b.py", "B\n");
        assert_eq!(review.undecided(&ws), vec!["b.py"]);
        let d = diff_working_set(&ws);
        review
            .record(&ws, &event(&d[1], FileDecision::Rejected), "e6")
            .unwrap();
        let plan = review.apply_plan(&ws).unwrap();
        assert_eq!(
            (plan.accepted.clone(), plan.rejected.clone()),
            (vec!["a.py".to_owned()], vec!["b.py".to_owned()])
        );
        assert_eq!(plan.files["b.py"].as_deref(), Some(b"b\n".as_slice()));
        assert_eq!(plan.post_image["b.py"], Some(hash(b"b\n")));
        assert_eq!(
            plan.change_set_sha256(),
            review.apply_plan(&ws).unwrap().change_set_sha256()
        );
        assert!(!format!("{plan:?}").contains("TWELVE"));
        // Revert to base → Rejected bound to restored content; nothing accepted.
        ws.revert_file("a.py").unwrap();
        review.mark_reverted(&ws, "a.py", "e7");
        assert_eq!(review.files["a.py"].decision, FileDecision::Rejected);
        assert_eq!(
            review.apply_plan(&ws).unwrap_err(),
            ReviewError::NothingAccepted
        );
        // Added files: whole-file decisions only.
        ws.apply_edit(
            &policy,
            &ProposedEdit::Create {
                path: "new/x.py".into(),
                content: "x\n".into(),
            },
        )
        .unwrap();
        let d = diff_working_set(&ws);
        let added = d.iter().find(|f| f.path == "new/x.py").unwrap();
        let partial = FileDecision::PartiallyAccepted {
            hunks: [0].into(),
            composed_sha256: String::new(),
        };
        assert_eq!(
            review.record(&ws, &event(added, partial), "e8"),
            Err(ReviewError::WholeFileOnly)
        );
        // Strict, content-free event JSON.
        let json = serde_json::to_string(&event(added, FileDecision::Accepted)).unwrap();
        assert!(serde_json::from_str::<UiReviewEvent>(&json).is_ok());
        let forged = json.replacen('{', r#"{"auto_accept":true,"#, 1);
        assert!(serde_json::from_str::<UiReviewEvent>(&forged).is_err());
        assert!(serde_json::from_str::<FileDecision>(
            r#"{"kind":"partially_accepted","hunks":[0],"composed_sha256":"","x":1}"#
        )
        .is_err());
    }

    /// Trusted host-side test oracle (never generated code): an independent
    /// strict unified-diff applier, and a difflib patch generator.
    #[cfg(target_os = "linux")]
    const PY_VERIFIER: &str = r#"
import difflib, json, os, re, sys
mode, root = sys.argv[1], sys.argv[2]
def split_keep(s):
    out, start = [], 0
    while True:
        j = s.find('\n', start)
        if j < 0:
            if start < len(s):
                out.append(s[start:])
            return out
        out.append(s[start:j + 1])
        start = j + 1
if mode == 'apply':
    tree = os.path.join(root, 'tree')
    lines = split_keep(open(os.path.join(root, 'task.patch'), 'rb').read().decode('utf-8'))
    hdr = re.compile(r'^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@')
    i, count = 0, 0
    while i < len(lines):
        if lines[i].startswith('Binary files '):
            i += 1
            continue
        if not (lines[i].startswith('--- ') and lines[i + 1].startswith('+++ ')):
            raise SystemExit('unexpected line %d %r' % (i, lines[i]))
        old = lines[i][4:].rstrip('\n').split('\t')[0]
        new = lines[i + 1][4:].rstrip('\n').split('\t')[0]
        i += 2
        assert old == '/dev/null' or old.startswith('a/'), old
        assert new == '/dev/null' or new.startswith('b/'), new
        rel = (new if new != '/dev/null' else old)[2:]
        path = os.path.join(tree, rel)
        if old == '/dev/null':
            assert not os.path.exists(path), rel
            src = ''
        else:
            src = open(path, 'rb').read().decode('utf-8')
        olines, out, cur, delta = split_keep(src), [], 0, 0
        while i < len(lines) and lines[i].startswith('@@ '):
            m = hdr.match(lines[i])
            i += 1
            os_, ol = int(m.group(1)), int(m.group(2) if m.group(2) is not None else 1)
            ns, nl = int(m.group(3)), int(m.group(4) if m.group(4) is not None else 1)
            opos = os_ if ol == 0 else os_ - 1
            npos = ns if nl == 0 else ns - 1
            assert opos >= cur and npos == opos + delta, ('position', rel)
            oside, nside, last = [], [], None
            while len(oside) < ol or len(nside) < nl or (i < len(lines) and lines[i].startswith('\\')):
                t = lines[i]
                i += 1
                if t.startswith('\\'):
                    for side in {' ': (oside, nside), '-': (oside,), '+': (nside,)}[last]:
                        assert side[-1].endswith('\n')
                        side[-1] = side[-1][:-1]
                    last = '\\'
                    continue
                tag, body = t[0], t[1:]
                if not body.endswith('\n'):
                    body += '\n'
                if tag in ' -':
                    oside.append(body)
                if tag in ' +':
                    nside.append(body)
                if tag not in ' -+':
                    raise SystemExit('bad hunk line %r' % t)
                last = tag
            assert (len(oside), len(nside)) == (ol, nl), ('counts', rel)
            assert olines[opos:opos + ol] == oside, ('context mismatch', rel, opos)
            out += olines[cur:opos] + nside
            cur, delta = opos + ol, delta + nl - ol
        result = ''.join(out + olines[cur:])
        if new == '/dev/null':
            assert result == '', rel
            os.remove(path)
        else:
            os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
            open(path, 'wb').write(result.encode('utf-8'))
        count += 1
    print('PY_INDEPENDENT_APPLY files=%d' % count)
elif mode == 'difflib':
    pairs = json.load(open(os.path.join(root, 'pairs.json')))
    out = {}
    for rel, (a, b) in pairs.items():
        diff = []
        for line in difflib.unified_diff(split_keep(a), split_keep(b), 'a/' + rel, 'b/' + rel):
            diff.append(line)
            if not line.endswith('\n'):
                diff.append('\n\\ No newline at end of file\n')
        out[rel] = ''.join(diff)
    json.dump(out, open(os.path.join(root, 'difflib.json'), 'w'))
    print('PY_DIFFLIB files=%d' % len(out))
"#;

    #[cfg(target_os = "linux")]
    fn host_tool(program: &str, args: &[&str]) -> (bool, String) {
        let out = std::process::Command::new(program)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }
    #[cfg(target_os = "linux")]
    fn write_tree(dir: &std::path::Path, files: &BTreeMap<String, Vec<u8>>) {
        for (p, b) in files {
            let path = dir.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b).unwrap();
        }
    }
    #[cfg(target_os = "linux")]
    fn read_tree(
        dir: &std::path::Path,
        paths: &BTreeSet<String>,
    ) -> BTreeMap<String, Option<Vec<u8>>> {
        paths
            .iter()
            .map(|p| (p.clone(), std::fs::read(dir.join(p)).ok()))
            .collect()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn task_diff_export_patch_applies_with_python_difflib_check() {
        let calc: String = (1..=40)
            .map(|i| format!("def f{i}():\n    return {i}\n"))
            .collect();
        let calc_new = calc
            .replace("return 3\n", "return 3 + 0\n")
            .replace("return 20\n", "return 20 * 1\n")
            .replace("def f39():\n", "def f39():\n    # fixed\n");
        let partial: String = (1..=30).map(|i| format!("p{i}\n")).collect();
        let partial_new = partial
            .replace("p2\n", "P2\n")
            .replace("p15\n", "P15\n")
            .replace("p28\n", "P28\n");
        let base: BTreeMap<String, Vec<u8>> = [
            ("src/calc.py", calc.as_str()),
            ("src/partial.py", partial.as_str()),
            ("src/crlf.py", "a = 1\r\nb = 2\r\nc = 3\r\n"),
            ("src/noeol.py", "x = 1\ny = 2"),
            ("src/remove.py", "gone = True\n"),
            ("README.md", "# readme\n"),
            ("src/space name.py", "s = 1\n"),
        ]
        .iter()
        .map(|(p, c)| ((*p).to_owned(), c.as_bytes().to_vec()))
        .collect();
        let mut ws = WorkingSet::from_parts(base.clone(), base.clone()).unwrap();
        let policy = PathPolicy::new(
            base.keys().cloned().collect(),
            BTreeSet::new(),
            vec!["src/".into()],
        );
        let edits = [
            ProposedEdit::Replace {
                path: "src/calc.py".into(),
                content: calc_new.clone(),
            },
            ProposedEdit::Replace {
                path: "src/partial.py".into(),
                content: partial_new.clone(),
            },
            ProposedEdit::Replace {
                path: "src/crlf.py".into(),
                content: "a = 1\r\nb = 20\r\nc = 3\r\n".into(),
            },
            ProposedEdit::Replace {
                path: "src/noeol.py".into(),
                content: "x = 1\ny = 3".into(),
            },
            ProposedEdit::Delete {
                path: "src/remove.py".into(),
            },
            ProposedEdit::Replace {
                path: "README.md".into(),
                content: "# changed but rejected\n".into(),
            },
            ProposedEdit::Create {
                path: "src/new_mod.py".into(),
                content: "NEW = 1\n".into(),
            },
            ProposedEdit::Replace {
                path: "src/space name.py".into(),
                content: "s = 2\n".into(),
            },
        ];
        for e in &edits {
            ws.apply_edit(&policy, e).unwrap();
        }
        let diffs = diff_working_set(&ws);
        assert_eq!(diffs.len(), 8);
        let mut review = ReviewState::default();
        for d in &diffs {
            let decision = match d.path.as_str() {
                "README.md" => FileDecision::Rejected,
                "src/partial.py" => {
                    assert_eq!(d.hunks.len(), 3);
                    FileDecision::PartiallyAccepted {
                        hunks: [0, 2].into(),
                        composed_sha256: String::new(),
                    }
                }
                _ => FileDecision::Accepted,
            };
            review
                .record(&ws, &event(d, decision), "synthetic-review-not-a-human")
                .unwrap();
        }
        let plan = review.apply_plan(&ws).unwrap();
        assert_eq!(plan.rejected, vec!["README.md"]);
        assert_eq!(
            plan.files["src/partial.py"].as_deref(),
            Some(
                partial
                    .replace("p2\n", "P2\n")
                    .replace("p28\n", "P28\n")
                    .as_bytes()
            )
        );
        let patch = export_patch(&diffs, &review);
        assert!(
            !patch.contains("rejected") && !patch.contains("P15"),
            "only accepted content is exported"
        );
        let paths: BTreeSet<String> = plan.files.keys().cloned().collect();
        // 1) Independent Python strict applier.
        let t = tempfile::tempdir().unwrap();
        let py = t.path().join("py");
        write_tree(&py.join("tree"), &base);
        std::fs::write(py.join("task.patch"), &patch).unwrap();
        let (ok, out) = host_tool(
            "/usr/bin/python3",
            &["-I", "-c", PY_VERIFIER, "apply", py.to_str().unwrap()],
        );
        println!("{out}");
        assert!(ok, "{out}");
        assert_eq!(
            read_tree(&py.join("tree"), &paths),
            plan.files,
            "python re-apply == accepted post-image"
        );
        // 2) GNU patch (host tool, strict: no fuzz).
        let gnu = t.path().join("gnu");
        write_tree(&gnu, &base);
        std::fs::write(t.path().join("task.patch"), &patch).unwrap();
        let (ok, out) = host_tool(
            "/usr/bin/patch",
            &[
                "-p1",
                "-E",
                "--batch",
                "--fuzz=0",
                "--no-backup-if-mismatch",
                "-d",
                gnu.to_str().unwrap(),
                "-i",
                t.path().join("task.patch").to_str().unwrap(),
            ],
        );
        println!("GNU_PATCH ok={ok} {out}");
        assert!(ok, "{out}");
        assert_eq!(
            read_tree(&gnu, &paths),
            plan.files,
            "GNU patch re-apply == accepted post-image"
        );
        // 3) Reverse independence: difflib-generated patches apply with our
        //    strict exact-context applier.
        let pairs: BTreeMap<&str, (String, String)> = diffs
            .iter()
            .filter(|d| d.change == Change::Modified)
            .map(|d| {
                let b = String::from_utf8(base[&d.path].clone()).unwrap();
                let c = String::from_utf8(ws.files()[&d.path].clone()).unwrap();
                (d.path.as_str(), (b, c))
            })
            .collect();
        let dl = t.path().join("difflib");
        std::fs::create_dir(&dl).unwrap();
        std::fs::write(dl.join("pairs.json"), serde_json::to_vec(&pairs).unwrap()).unwrap();
        let (ok, out) = host_tool(
            "/usr/bin/python3",
            &["-I", "-c", PY_VERIFIER, "difflib", dl.to_str().unwrap()],
        );
        println!("{out}");
        assert!(ok, "{out}");
        let generated: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(dl.join("difflib.json")).unwrap()).unwrap();
        assert_eq!(generated.len(), pairs.len());
        for (path, unified) in &generated {
            let mut fresh = WorkingSet::from_parts(base.clone(), base.clone()).unwrap();
            fresh
                .apply_edit(
                    &policy,
                    &ProposedEdit::Patch {
                        path: path.clone(),
                        unified_hunks: unified.clone(),
                    },
                )
                .unwrap_or_else(|e| panic!("{path}: {e:?}\n{unified}"));
            assert_eq!(fresh.files()[path], ws.files()[path], "{path}");
        }
        // Stale or tampered review entries are never exported.
        let mut stale = review.clone();
        stale
            .files
            .get_mut("src/calc.py")
            .unwrap()
            .reviewed_new_sha256 = Some(hash(b"old"));
        assert!(!export_patch(&diffs, &stale).contains("src/calc.py"));
        let mut tampered = diffs.clone();
        let calc_diff = tampered
            .iter_mut()
            .find(|d| d.path == "src/calc.py")
            .unwrap();
        calc_diff.hunks[0].lines[0].text = "injected\n".into();
        assert!(!export_patch(&tampered, &review).contains("src/calc.py"));
    }
}
