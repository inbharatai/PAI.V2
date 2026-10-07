//! Stage 6 bounded local distiller (owner K1).
//!
//! Deterministic extractive distillation ONLY: Markdown/text headings plus their
//! first sentence or bullet, Python top-level `def`/`class` signatures plus the
//! first docstring line, otherwise the first non-empty lines. No model, no
//! network, no training export and no promotion: a run writes Evidence
//! (Artifact chunks + one Observation run summary) and Candidate records through
//! the existing Stage 2 `EvidenceVault`, nothing else. Every candidate cites
//! exact byte offsets in the stored evidence content.
//!
//! Source text is untrusted data: it is stored verbatim inside encryption, and
//! every derived display string (statement, excerpt, topic) has control and
//! bidirectional-formatting characters replaced and whitespace collapsed.
//!
//! Stage 3 index safety: the postings index has fixed per-document and global
//! caps and the store is append-only (records can never be removed), so one
//! over-cap record would make `rebuild_index` fail forever. Sources are therefore
//! split at line/whitespace boundaries into Artifact chunks that each stay within
//! the per-document term cap, and the run stops (`budget`) before the projected
//! index would exceed 75% of the global caps. The tokenizer below mirrors Stage 3
//! `lexical_terms` exactly; `rebuild_index` after real distillation is tested.

use crate::knowledge::{self, EvidenceVault};
use crate::knowledge_retrieval::{ApplicabilityFence, FENCE_SCHEMA};
use crate::knowledge_service::{store_error, KnowledgeError, KnowledgeServiceConfig, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use unoone_capability_contracts::knowledge::*;

pub const DISTILL_METHOD: &str =
    "extractive-headings-docstrings-v1; deterministic; no model; no network";
pub const RUN_SUMMARY_SCHEMA: &str = "inbharat.pai.knowledge-distill-run.v1";
pub const MAX_SOURCES: usize = 32;
pub const MAX_TOTAL_BYTES: usize = 1024 * 1024;
pub const MAX_CANDIDATES: usize = 64;
pub const MAX_DEADLINE_MS: u64 = 30_000;
const MAX_LABEL_CHARS: usize = 128;
const MAX_LICENSE_BYTES: usize = 256;
const MAX_PLATFORM_BYTES: usize = 128;
const MAX_TOPICS: usize = 8;
const MAX_TOPIC_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 256;
pub(crate) const MAX_STATEMENT_CHARS: usize = 280;
pub(crate) const MAX_EXCERPT_CHARS: usize = 280;
/// Chunk content bound: far below the 64 KiB contract bound (JSON escaping
/// headroom inside the 256 KiB record bound) and below the 16 Ki-scalar detail
/// view bound, so a stored chunk is always shown untruncated.
pub(crate) const MAX_CHUNK_BYTES: usize = 16 * 1024;
const MAX_SENTENCE_SCAN: usize = 1024;
const MAX_SIGNATURE_LINES: usize = 8;
const MAX_CONTRADICTIONS_PER_CANDIDATE: usize = 8;
// Mirrors of the fixed private Stage 3 index caps (knowledge_retrieval.rs).
pub(crate) const INDEX_MAX_DOCUMENT_TERMS: usize = 512;
pub(crate) const INDEX_MAX_TERM_CHARS: usize = 256;
const INDEX_MAX_POSTINGS: usize = 262_144;
const INDEX_MAX_TERMS: usize = 65_536;
const INDEX_GUARD_POSTINGS: usize = INDEX_MAX_POSTINGS / 4 * 3;
const INDEX_GUARD_TERMS: usize = INDEX_MAX_TERMS / 4 * 3;
const CATALOG_GUARD: usize = knowledge::MAX_CATALOG_ENTRIES - 64;
const _: () =
    assert!(INDEX_GUARD_POSTINGS < INDEX_MAX_POSTINGS && INDEX_GUARD_TERMS < INDEX_MAX_TERMS);

// ------------------------------------------------------------------- types

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistillRequest {
    /// 1..=32 sources.
    pub sources: Vec<DistillSource>,
    pub budget: DistillBudget,
    /// Applicability platform for produced records.
    pub platform: String,
    /// User-declared licence, <= 256 bytes; "unknown" allowed.
    pub license: String,
    /// Optional extra topics, <= 8.
    pub topics: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DistillSource {
    /// `label` <= 128 scalars.
    PastedText { label: String, text: String },
    /// `root` MUST be a granted folder (glue checks); read fd-safely.
    LocalFile { root: PathBuf, path: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistillBudget {
    /// 1..=1 MiB.
    pub max_total_bytes: usize,
    /// 1..=64.
    pub max_candidates: usize,
    /// 1..=30_000.
    pub deadline_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiDistillEvent {
    pub request_sha256: String,
    pub ui_event_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DistillReport {
    pub run_id: String,
    /// Always `DISTILL_METHOD`.
    pub method: String,
    pub evidence: Vec<RecordRef>,
    pub candidates: Vec<DistilledCandidate>,
    pub excluded: Vec<ExcludedSource>,
    pub possible_contradictions: Vec<ContradictionView>,
    pub budget_exhausted: bool,
    pub elapsed_ms: u64,
    /// Observation evidence of the run.
    pub summary: RecordRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DistilledCandidate {
    pub reference: RecordRef,
    pub statement: String,
    pub citation: Citation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Citation {
    pub evidence: RecordRef,
    /// Byte offsets in the evidence content (`content[start..end]`).
    pub start: usize,
    pub end: usize,
    /// Display-sanitized prefix of `content[start..end]`, <= 280 scalars.
    pub excerpt: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExcludedSource {
    pub label: String,
    /// held_out_hash | held_out_name | duplicate | too_large | unreadable | not_utf8 | budget
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContradictionView {
    pub candidate: RecordRef,
    pub existing: RecordRef,
    /// Heuristic rule id.
    pub rule: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DistillRunSummary {
    pub summary: RecordRef,
    pub run_id: String,
    pub timestamp_ms: u64,
    pub evidence: usize,
    pub candidates: usize,
    pub excluded: usize,
}

/// Durable run summary body (Observation evidence content). Counts, method and
/// exclusion reasons only: no source text, no labels (label SHA-256 prefixes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunSummaryBody {
    schema: String,
    run_id: String,
    method: String,
    timestamp_ms: u64,
    request_sha256: String,
    evidence: usize,
    candidates: usize,
    excluded: Vec<SummaryExclusion>,
    possible_contradictions: usize,
    budget_exhausted: bool,
    elapsed_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SummaryExclusion {
    label_sha256: String,
    reason: String,
}

// -------------------------------------------------------- request checking

fn plain_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

pub(crate) fn validate_request(request: &DistillRequest) -> Result<(), KnowledgeError> {
    let invalid = Err(KnowledgeError::Invalid);
    let budget = &request.budget;
    if request.sources.is_empty()
        || request.sources.len() > MAX_SOURCES
        || !(1..=MAX_TOTAL_BYTES).contains(&budget.max_total_bytes)
        || !(1..=MAX_CANDIDATES).contains(&budget.max_candidates)
        || !(1..=MAX_DEADLINE_MS).contains(&budget.deadline_ms)
        || !plain_text(&request.platform, MAX_PLATFORM_BYTES)
        || !plain_text(&request.license, MAX_LICENSE_BYTES)
        || request.topics.len() > MAX_TOPICS
        || request.topics.iter().collect::<BTreeSet<_>>().len() != request.topics.len()
        || request
            .topics
            .iter()
            .any(|t| !plain_text(t, MAX_TOPIC_BYTES))
    {
        return invalid;
    }
    for source in &request.sources {
        let ok = match source {
            DistillSource::PastedText { label, text } => {
                plain_text(label, MAX_LABEL_CHARS * 4)
                    && label.chars().count() <= MAX_LABEL_CHARS
                    && !text.trim().is_empty()
                    && text.len() <= MAX_TOTAL_BYTES
            }
            DistillSource::LocalFile { root, path } => {
                root.is_absolute() && root.to_str().is_some() && plain_text(path, MAX_PATH_BYTES)
            }
        };
        if !ok {
            return invalid;
        }
    }
    Ok(())
}

/// `sha256(canonical serde_json of DistillRequest)` after bound validation. The
/// UI shows the plan + this hash; `distill` refuses unless the event echoes it.
/// A non-UTF-8 `root` cannot be serialized and is `Invalid`.
pub fn distill_request_sha256(request: &DistillRequest) -> Result<String, KnowledgeError> {
    validate_request(request)?;
    let bytes = serde_json::to_vec(request).map_err(|_| KnowledgeError::Invalid)?;
    Ok(knowledge::digest(&bytes))
}

// ---------------------------------------------------- text/term utilities

/// EXACT mirror of the private Stage 3 `knowledge_retrieval::lexical_terms`
/// (Unicode scalar lowercase; alphanumeric, '_' and ':' term characters).
pub(crate) fn lexical_terms(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut term = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            term.push(c);
        } else if !term.is_empty() {
            result.push(std::mem::take(&mut term));
        }
    }
    if !term.is_empty() {
        result.push(term);
    }
    result
}
fn term_set(text: &str) -> BTreeSet<String> {
    lexical_terms(text).into_iter().collect()
}
/// Stage 3 document features (`lexical_weights` keys): body, topics, logical id;
/// invalidations contribute no postings.
pub(crate) fn index_terms(body: &str, topics: &[String], logical_id: &str) -> BTreeSet<String> {
    let mut terms = term_set(body);
    for topic in topics {
        terms.extend(lexical_terms(topic));
    }
    terms.extend(lexical_terms(logical_id));
    terms
}
pub(crate) fn record_index_terms(record: &KnowledgeRecord) -> BTreeSet<String> {
    let body = match record {
        KnowledgeRecord::Evidence(x) => x.content.as_str(),
        KnowledgeRecord::Candidate(x) => x.statement.as_str(),
        KnowledgeRecord::VerifiedPattern(x) => x.statement.as_str(),
        KnowledgeRecord::ApprovedProcedure(x) => x.outcome.postconditions.as_str(),
        KnowledgeRecord::Invalidation(_) => return BTreeSet::new(),
    };
    let header = record.header();
    index_terms(
        body,
        &header.metadata.applicability.topics,
        &header.logical_id,
    )
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}
/// Untrusted source text for display: control and bidi formatting characters
/// become spaces (never merging two terms), whitespace collapses, <= max scalars.
pub(crate) fn display_text(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut pending_space = false;
    for c in value.chars() {
        let c = if c.is_control() || is_bidi_control(c) {
            ' '
        } else {
            c
        };
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            if count + 1 >= max_chars {
                break;
            }
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        if count >= max_chars {
            break;
        }
        out.push(c);
        count += 1;
    }
    out
}
/// Dedup key: lowercase alphanumeric words joined by single spaces.
pub(crate) fn normalize_statement(statement: &str) -> String {
    statement
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn random_hex() -> Result<String, KnowledgeError> {
    // OS randomness via the existing vault crypto (works on every target).
    let nonce = unoone_vault_core::crypto::generate_nonce();
    Ok(nonce[..16].iter().map(|b| format!("{b:02x}")).collect())
}
pub(crate) fn now_ms() -> Result<u64, KnowledgeError> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| KnowledgeError::Persistence)?
        .as_millis();
    u64::try_from(ms)
        .ok()
        .filter(|v| *v > 0)
        .ok_or(KnowledgeError::Persistence)
}

// -------------------------------------------------------------- extraction

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocType {
    Markdown,
    Python,
    Other,
}
pub(crate) fn extension(name: &str) -> Option<String> {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = file.rsplit_once('.')?;
    if stem.is_empty()
        || ext.is_empty()
        || ext.len() > 16
        || !ext.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}
pub(crate) fn doc_type(name: &str) -> DocType {
    match extension(name).as_deref() {
        None | Some("md" | "markdown" | "mdx" | "txt" | "text" | "rst" | "adoc") => {
            DocType::Markdown
        }
        Some("py" | "pyi") => DocType::Python,
        Some(_) => DocType::Other,
    }
}

/// One extracted claim: `text[start..end]` is the exact cited span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Extracted {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) statement: String,
    pub(crate) anchor: String,
}
#[derive(Debug, Clone, Copy)]
struct Line {
    start: usize,
    /// Excludes "\n" / "\r\n".
    end: usize,
}
fn lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    for piece in text.split_inclusive('\n') {
        let mut end = start + piece.len();
        if piece.ends_with('\n') {
            end -= 1;
            if text[start..end].ends_with('\r') {
                end -= 1;
            }
        }
        out.push(Line { start, end });
        start += piece.len();
    }
    out
}
fn heading(line: &str) -> Option<&str> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &rest[hashes..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let text = after.trim().trim_end_matches('#').trim();
    (!text.is_empty()).then_some(text)
}
fn fence_marker(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}
fn bullet(trimmed: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest);
        }
    }
    let digits = trimmed.len()
        - trimmed
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .len();
    if (1..=3).contains(&digits) {
        let rest = &trimmed[digits..];
        for marker in [". ", ") "] {
            if let Some(rest) = rest.strip_prefix(marker) {
                return Some(rest);
            }
        }
    }
    None
}
fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}
/// First sentence or bullet after heading line `i`: (text, span end).
fn first_sentence(text: &str, lines: &[Line], from: usize) -> Option<(String, usize)> {
    let mut k = from;
    let mut fence: Option<&str> = None;
    while k < lines.len() {
        let raw = &text[lines[k].start..lines[k].end];
        let trimmed = raw.trim();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            k += 1;
            continue;
        }
        if trimmed.is_empty() {
            k += 1;
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            k += 1;
            continue;
        }
        if heading(raw).is_some() {
            return None;
        }
        if let Some(rest) = bullet(trimmed) {
            return Some((rest.to_string(), lines[k].end));
        }
        // Paragraph: consecutive non-blank, non-heading, non-fence, non-bullet lines.
        let start = lines[k].start + (raw.len() - raw.trim_start().len());
        let mut m = k + 1;
        while m < lines.len() {
            let next = &text[lines[m].start..lines[m].end];
            let t = next.trim();
            if t.is_empty()
                || heading(next).is_some()
                || fence_marker(t).is_some()
                || bullet(t).is_some()
            {
                break;
            }
            m += 1;
        }
        let region_end = lines[m - 1].end;
        let scan_end = floor_boundary(text, region_end.min(start + MAX_SENTENCE_SCAN));
        let region = &text[start..scan_end];
        let mut end = None;
        let mut chars = region.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if matches!(c, '.' | '!' | '?') {
                let at_end = start + i + c.len_utf8() >= region_end;
                if at_end || chars.peek().is_some_and(|(_, n)| n.is_whitespace()) {
                    end = Some(start + i + c.len_utf8());
                    break;
                }
            }
        }
        let end = end.unwrap_or(if region_end <= start + MAX_SENTENCE_SCAN {
            region_end
        } else {
            floor_boundary(text, lines[k].end.min(start + MAX_SENTENCE_SCAN))
        });
        return Some((text[start..end].to_string(), end));
    }
    None
}
fn extract_markdown(text: &str) -> Vec<Extracted> {
    let lines = lines(text);
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    for (i, line) in lines.iter().enumerate() {
        let raw = &text[line.start..line.end];
        let trimmed = raw.trim();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            continue;
        }
        let Some(head) = heading(raw) else { continue };
        if let Some((sentence, end)) = first_sentence(text, &lines, i + 1) {
            let statement = display_text(&format!("{head}: {sentence}"), MAX_STATEMENT_CHARS);
            if !statement.is_empty() {
                out.push(Extracted {
                    start: line.start,
                    end,
                    statement,
                    anchor: display_text(head, 64),
                });
            }
        }
    }
    out
}
fn strip_comment(line: &str) -> &str {
    line.split_once('#').map_or(line, |(code, _)| code)
}
fn extract_python(text: &str) -> Vec<Extracted> {
    let lines = lines(text);
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let raw = &text[line.start..line.end];
        let keyword = ["def ", "async def ", "class "]
            .into_iter()
            .find(|k| raw.starts_with(k));
        let Some(keyword) = keyword else { continue };
        let name: String = raw[keyword.len()..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        let mut last = i;
        while last < lines.len() && last < i + MAX_SIGNATURE_LINES {
            let l = &text[lines[last].start..lines[last].end];
            if strip_comment(l).trim_end().ends_with(':') {
                break;
            }
            last += 1;
        }
        if last >= lines.len() || last >= i + MAX_SIGNATURE_LINES {
            last = i;
        }
        let signature: Vec<&str> = (i..=last)
            .map(|k| strip_comment(&text[lines[k].start..lines[k].end]).trim())
            .collect();
        let signature = display_text(&signature.join(" "), MAX_STATEMENT_CHARS);
        let signature = signature.trim_end_matches(':').trim_end().to_string();
        let mut end = lines[last].end;
        let mut doc = String::new();
        if let Some(k) = (last + 1..lines.len())
            .find(|k| !text[lines[*k].start..lines[*k].end].trim().is_empty())
        {
            let t = text[lines[k].start..lines[k].end].trim();
            let unprefixed = t.trim_start_matches(['r', 'R', 'u', 'U', 'b', 'B', 'f', 'F']);
            let quote = ["\"\"\"", "'''"]
                .into_iter()
                .find(|q| unprefixed.starts_with(q));
            if let Some(quote) = quote {
                let rest = &unprefixed[quote.len()..];
                let first = rest.split(quote).next().unwrap_or("").trim();
                if !first.is_empty() {
                    doc = first.to_string();
                    end = lines[k].end;
                } else if !rest.contains(quote) {
                    if let Some(next) = lines.get(k + 1) {
                        let n = text[next.start..next.end].trim();
                        let n = n.split(quote).next().unwrap_or("").trim();
                        if !n.is_empty() {
                            doc = n.to_string();
                            end = next.end;
                        }
                    }
                }
            }
        }
        let statement = if doc.is_empty() {
            signature
        } else {
            display_text(&format!("{signature}: {doc}"), MAX_STATEMENT_CHARS)
        };
        if !statement.is_empty() {
            out.push(Extracted {
                start: line.start,
                end,
                statement,
                anchor: name,
            });
        }
    }
    out
}
fn extract_first_lines(text: &str) -> Vec<Extracted> {
    let picked: Vec<Line> = lines(text)
        .into_iter()
        .filter(|l| !text[l.start..l.end].trim().is_empty())
        .take(3)
        .collect();
    let (Some(first), Some(last)) = (picked.first(), picked.last()) else {
        return vec![];
    };
    let joined: Vec<&str> = picked.iter().map(|l| text[l.start..l.end].trim()).collect();
    let statement = display_text(&joined.join(" "), MAX_STATEMENT_CHARS);
    if statement.is_empty() {
        return vec![];
    }
    let anchor = display_text(&text[first.start..first.end], 64);
    vec![Extracted {
        start: first.start,
        end: last.end,
        statement,
        anchor,
    }]
}
/// Deterministic extraction in source order (span coordinates = source bytes).
pub(crate) fn extract(text: &str, doc: DocType) -> Vec<Extracted> {
    let found = match doc {
        DocType::Markdown => extract_markdown(text),
        DocType::Python => extract_python(text),
        DocType::Other => vec![],
    };
    if found.is_empty() {
        extract_first_lines(text)
    } else {
        found
    }
}

// ---------------------------------------------------------------- chunking

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chunking {
    /// Byte ranges of the source; every range holds non-whitespace text.
    pub(crate) chunks: Vec<(usize, usize)>,
    /// Source tail from here on is not stored (a term longer than the Stage 3
    /// term cap, or a whitespace-free run over a chunk budget): reported too_large.
    pub(crate) truncated_at: Option<usize>,
}
/// Greedy line-based chunks within MAX_CHUNK_BYTES and the Stage 3 per-document
/// term cap (topics and the run/chunk logical id included). Cuts never fall
/// strictly inside an extracted span unless a single span exceeds a budget.
pub(crate) fn chunk_source(
    text: &str,
    spans: &[(usize, usize)],
    topics: &[String],
    id_prefix: &str,
) -> Chunking {
    let mut base: BTreeSet<String> = topics.iter().flat_map(|t| lexical_terms(t)).collect();
    base.extend(lexical_terms(id_prefix));
    // +1: the per-chunk "eNNN" component of the logical id.
    let budget = INDEX_MAX_DOCUMENT_TERMS.saturating_sub(base.len() + 1);
    let fits = |piece: &str, terms: &BTreeSet<String>| {
        piece.len() <= MAX_CHUNK_BYTES && terms.len() <= budget
    };
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut truncated_at = None;
    let mut start = 0;
    'lines: for line in text.split_inclusive('\n') {
        let (ls, le) = (start, start + line.len());
        start = le;
        let terms = term_set(line);
        if terms
            .iter()
            .any(|t| t.chars().count() > INDEX_MAX_TERM_CHARS)
        {
            truncated_at = Some(ls);
            break;
        }
        if fits(line, &terms) {
            pieces.push((ls, le));
            continue;
        }
        let mut ps = ls;
        let mut subs = Vec::new();
        for (i, c) in line.char_indices() {
            if c.is_whitespace() {
                let pe = ls + i + c.len_utf8();
                subs.push((ps, pe));
                ps = pe;
            }
        }
        if ps < le {
            subs.push((ps, le));
        }
        for (s, e) in subs {
            if !fits(&text[s..e], &term_set(&text[s..e])) {
                truncated_at = Some(s);
                break 'lines;
            }
            pieces.push((s, e));
        }
    }
    let mut chunks = Vec::new();
    let mut current: Option<usize> = None;
    let mut acc: BTreeSet<String> = BTreeSet::new();
    let mut i = 0;
    while i < pieces.len() {
        let (ps, pe) = pieces[i];
        let chunk_start = current.unwrap_or(ps);
        let terms = term_set(&text[ps..pe]);
        let merged = acc.len() + terms.iter().filter(|t| !acc.contains(*t)).count();
        if pe - chunk_start <= MAX_CHUNK_BYTES && merged <= budget {
            acc.extend(terms);
            current = Some(chunk_start);
            i += 1;
            continue;
        }
        // A single piece always fits alone, so the open chunk is non-empty here.
        let mut cut = ps;
        if let Some(&(span_start, _)) = spans.iter().find(|(s, e)| *s < ps && ps < *e) {
            if span_start > chunk_start {
                if let Some(j) = pieces.iter().position(|(s, _)| *s == span_start) {
                    cut = span_start;
                    i = j;
                }
            }
        }
        chunks.push((chunk_start, cut));
        current = None;
        acc.clear();
    }
    if let (Some(chunk_start), Some(&(_, end))) = (current, pieces.last()) {
        chunks.push((chunk_start, end));
    }
    chunks.retain(|(s, e)| !text[*s..*e].trim().is_empty());
    Chunking {
        chunks,
        truncated_at,
    }
}

// ---------------------------------------------------- contradiction heuristic

pub const RULE_NEGATION: &str = "shared-terms>=3+negation-mismatch-v1";
pub const RULE_NUMBER: &str = "shared-terms>=3+number-mismatch-v1";
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "are", "was", "were", "has", "have", "had", "but",
    "from", "into", "onto", "its", "it's", "can", "may", "must", "should", "shall", "will",
    "would", "could", "use", "uses", "used", "any", "all", "each", "per", "than", "then", "when",
    "while", "during", "via", "also", "only", "not", "never", "don't", "dont", "does", "did",
    "you", "your", "our", "their", "them", "they", "there", "here", "what", "which", "who", "how",
    "why", "been", "being", "more", "most", "less", "such",
];
fn words(statement: &str) -> Vec<String> {
    statement
        .to_lowercase()
        .replace('\u{2019}', "'")
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '.'))
        .map(|w| w.trim_matches(|c| c == '\'' || c == '.').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}
fn is_number(word: &str) -> bool {
    word.parse::<f64>().is_ok() && word.chars().any(|c| c.is_ascii_digit())
}
fn topic_terms(statement: &str) -> BTreeSet<String> {
    words(statement)
        .into_iter()
        .flat_map(|w| w.split(['\'', '.']).map(str::to_string).collect::<Vec<_>>())
        .filter(|w| {
            w.chars().count() >= 3
                && w.chars().any(char::is_alphabetic)
                && !STOPWORDS.contains(&w.as_str())
                && !w.starts_with("disable")
        })
        .collect()
}
fn has_negation(statement: &str) -> bool {
    words(statement)
        .iter()
        .any(|w| w == "not" || w == "never" || w == "don't" || w.starts_with("disable"))
}
fn numbers_by_key(statement: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut key: Option<String> = None;
    for word in words(statement) {
        if is_number(&word) {
            if let Some(k) = &key {
                out.entry(k.clone()).or_default().insert(word);
            }
        } else if word.chars().count() >= 3 && !STOPWORDS.contains(&word.as_str()) {
            key = Some(word);
        }
    }
    out
}
/// Labelled lexical heuristic, NOT semantic: >= 3 shared normalized topic terms
/// and (exactly one negation marker, or different numbers for one key term).
pub(crate) fn contradiction_rule(new: &str, existing: &str) -> Option<&'static str> {
    if topic_terms(new)
        .intersection(&topic_terms(existing))
        .count()
        < 3
    {
        return None;
    }
    if has_negation(new) != has_negation(existing) {
        return Some(RULE_NEGATION);
    }
    let (a, b) = (numbers_by_key(new), numbers_by_key(existing));
    if a.iter()
        .any(|(k, numbers)| b.get(k).is_some_and(|other| other != numbers))
    {
        return Some(RULE_NUMBER);
    }
    None
}

// -------------------------------------------------------------------- run

fn source_label(source: &DistillSource) -> &str {
    match source {
        DistillSource::PastedText { label, .. } => label,
        DistillSource::LocalFile { path, .. } => path,
    }
}
fn held_out_name(markers: &[String], source: &DistillSource) -> bool {
    let name = match source {
        DistillSource::PastedText { label, .. } => label.to_lowercase(),
        DistillSource::LocalFile { root, path } => {
            format!("{}/{}", root.to_string_lossy(), path).to_lowercase()
        }
    };
    markers
        .iter()
        .map(|m| m.trim().to_lowercase())
        .any(|m| !m.is_empty() && name.contains(&m))
}
fn read_source(config: &KnowledgeServiceConfig, source: &DistillSource) -> Result<Vec<u8>, ()> {
    match source {
        DistillSource::PastedText { text, .. } => Ok(text.as_bytes().to_vec()),
        DistillSource::LocalFile { root, path } => read_local(config, root, path),
    }
}
/// fd-safe single-file capture (Stage 5 `capture_selection` under a fresh
/// Stage 4 `SnapshotPolicy` for exactly this root): regular file, nlink == 1,
/// no symlink at any component, size bounds. The staged copy is dropped at once.
#[cfg(target_os = "linux")]
fn read_local(
    config: &KnowledgeServiceConfig,
    root: &std::path::Path,
    path: &str,
) -> Result<Vec<u8>, ()> {
    use crate::isolation::workspace::{capture_selection, SelectionLabel};
    use crate::isolation::SnapshotPolicy;
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&config.scratch)
        .map_err(|_| ())?;
    let policy =
        SnapshotPolicy::new(vec![root.to_path_buf()], config.scratch.clone()).map_err(|_| ())?;
    let label = SelectionLabel {
        source_id: "local:distill-capture".into(),
    };
    let captured =
        capture_selection(&policy, root, &label, path, &[path.to_string()]).map_err(|_| ())?;
    captured.files.get(path).cloned().ok_or(())
}
/// No fd-relative capture exists off Linux: LocalFile is excluded `unreadable`
/// ("fd-safe capture unavailable on this platform"); PastedText still works.
#[cfg(not(target_os = "linux"))]
fn read_local(
    _config: &KnowledgeServiceConfig,
    _root: &std::path::Path,
    _path: &str,
) -> Result<Vec<u8>, ()> {
    Err(())
}

fn derive_topics(request: &DistillRequest, name: &str, extracted: &[Extracted]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |topic: String| {
        if !topic.is_empty() && topic.len() <= 256 && !out.contains(&topic) {
            out.push(topic);
        }
    };
    for topic in &request.topics {
        push(display_text(topic, MAX_TOPIC_BYTES));
    }
    if let Some(ext) = extension(name) {
        push(ext);
    }
    for anchor in extracted.iter().take(4) {
        push(display_text(&anchor.anchor, 64));
    }
    if out.is_empty() {
        out.push("distilled".into());
    }
    out
}
fn source_metadata(
    request: &DistillRequest,
    source: &DistillSource,
    sha: &str,
    topics: Vec<String>,
) -> Result<KnowledgeMetadata, KnowledgeError> {
    let key = match source {
        DistillSource::PastedText { label, .. } => label.clone(),
        DistillSource::LocalFile { root, path } => {
            format!("{}\n{}", root.to_string_lossy(), path)
        }
    };
    let constraints = ApplicabilityFence {
        schema: FENCE_SCHEMA.into(),
        file_digest: sha.into(),
    }
    .encode()
    .map_err(|_| KnowledgeError::Invalid)?;
    Ok(KnowledgeMetadata {
        source_id: format!("local:{}", &knowledge::digest(key.as_bytes())[..16]),
        source_version: sha.into(),
        source_commit: sha.into(),
        license: request.license.clone(),
        privacy: KnowledgePrivacy::Private,
        applicability: Applicability {
            topics,
            platforms: vec![request.platform.clone()],
            constraints,
        },
    })
}
fn header(
    logical_id: String,
    timestamp_ms: u64,
    metadata: KnowledgeMetadata,
    edges: Vec<GraphEdge>,
    reason: String,
) -> RecordHeader {
    RecordHeader {
        schema: KNOWLEDGE_SCHEMA.into(),
        logical_id,
        revision: 1,
        previous: None,
        timestamp_ms,
        audit: Audit {
            actor: "knowledge-distiller".into(),
            reason,
        },
        metadata,
        edges,
    }
}

/// Projected Stage 3 index load (mirror of build_index accounting).
struct IndexLoad {
    postings: usize,
    terms: BTreeSet<String>,
    entries: usize,
}
impl IndexLoad {
    fn from_snapshot(snapshot: &Snapshot) -> Self {
        let mut load = Self {
            postings: 0,
            terms: BTreeSet::new(),
            entries: snapshot.entries.len(),
        };
        for item in snapshot.loaded.values() {
            let terms = record_index_terms(&item.record);
            load.postings += terms.len();
            load.terms.extend(terms);
        }
        load
    }
    fn admits(&self, terms: &BTreeSet<String>) -> bool {
        terms.len() <= INDEX_MAX_DOCUMENT_TERMS
            && terms
                .iter()
                .all(|t| t.chars().count() <= INDEX_MAX_TERM_CHARS)
            && self.postings + terms.len() <= INDEX_GUARD_POSTINGS
            && self.terms.len() + terms.iter().filter(|t| !self.terms.contains(*t)).count()
                <= INDEX_GUARD_TERMS
            // +1 this record, +1 reserved for the run summary.
            && self.entries + 2 <= CATALOG_GUARD
    }
    fn add(&mut self, terms: BTreeSet<String>) {
        self.postings += terms.len();
        self.terms.extend(terms);
        self.entries += 1;
    }
}

pub(crate) struct RunContext<'a> {
    pub(crate) store: &'a EvidenceVault,
    pub(crate) config: &'a KnowledgeServiceConfig,
    pub(crate) request_sha256: &'a str,
    pub(crate) ui_event_id: &'a str,
    /// Milliseconds since the run started (injectable for deterministic tests).
    pub(crate) elapsed_ms: &'a mut dyn FnMut() -> u64,
    /// False once the service observed a lock (epoch changed): abort before writes.
    pub(crate) alive: &'a dyn Fn() -> bool,
}

/// Executes one confirmed request. Caller validated the request hash/event and
/// supplies an authenticated snapshot taken at run start ("existing" records).
pub(crate) fn run(
    ctx: &mut RunContext<'_>,
    request: &DistillRequest,
    snapshot: &Snapshot,
) -> Result<DistillReport, KnowledgeError> {
    validate_request(request)?;
    let run_hex = random_hex()?;
    let run_id = format!("distill-{run_hex}");
    let timestamp_ms = now_ms()?;
    let deadline = request.budget.deadline_ms;
    let reason = format!(
        "{DISTILL_METHOD}; run {run_id}; ui_event {}",
        ctx.ui_event_id
    );
    let mut evidence_refs: Vec<RecordRef> = Vec::new();
    let mut candidates: Vec<DistilledCandidate> = Vec::new();
    let mut excluded: Vec<ExcludedSource> = Vec::new();
    let mut contradictions: Vec<ContradictionView> = Vec::new();
    let mut exhausted = false;
    let mut stop = false;
    // "Existing" state as of run start.
    let mut existing_sources = BTreeSet::new();
    let mut seen_statements = BTreeSet::new();
    let mut existing_heads: Vec<(RecordRef, String)> = Vec::new();
    for item in snapshot.loaded.values() {
        let reference = &item.mapping.reference;
        match &item.record {
            KnowledgeRecord::Evidence(e) if e.evidence_kind == EvidenceKind::Artifact => {
                existing_sources.insert(e.content_sha256.clone());
                existing_sources.insert(e.header.metadata.source_version.clone());
            }
            KnowledgeRecord::Candidate(Candidate { statement, .. })
            | KnowledgeRecord::VerifiedPattern(VerifiedPattern { statement, .. }) => {
                seen_statements.insert(normalize_statement(statement));
                let flags = snapshot.flags_of(reference)?;
                if snapshot.is_head(reference) && flags.active {
                    existing_heads.push((reference.clone(), statement.clone()));
                }
            }
            _ => {}
        }
    }
    let mut load = IndexLoad::from_snapshot(snapshot);
    let mut used_bytes = 0usize;
    let mut seen_sources = BTreeSet::new();
    let (mut evidence_seq, mut candidate_seq) = (0usize, 0usize);
    let exclude = |excluded: &mut Vec<ExcludedSource>, label: &str, reason: &str| {
        excluded.push(ExcludedSource {
            label: display_text(label, MAX_LABEL_CHARS),
            reason: reason.into(),
        });
    };
    for source in &request.sources {
        let label = source_label(source);
        if stop || (ctx.elapsed_ms)() >= deadline {
            exhausted = true;
            stop = true;
            exclude(&mut excluded, label, "budget");
            continue;
        }
        if held_out_name(&ctx.config.excluded_name_markers, source) {
            exclude(&mut excluded, label, "held_out_name");
            continue;
        }
        let Ok(bytes) = read_source(ctx.config, source) else {
            exclude(&mut excluded, label, "unreadable");
            continue;
        };
        let sha = knowledge::digest(&bytes);
        if ctx.config.excluded_sha256.contains(&sha) {
            exclude(&mut excluded, label, "held_out_hash");
            continue;
        }
        if bytes.len() > request.budget.max_total_bytes {
            exclude(&mut excluded, label, "too_large");
            continue;
        }
        if used_bytes + bytes.len() > request.budget.max_total_bytes {
            exhausted = true;
            exclude(&mut excluded, label, "budget");
            continue;
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => text,
            _ => {
                exclude(&mut excluded, label, "not_utf8");
                continue;
            }
        };
        if seen_sources.contains(&sha) || existing_sources.contains(&sha) {
            exclude(&mut excluded, label, "duplicate");
            continue;
        }
        seen_sources.insert(sha.clone());
        used_bytes += text.len();
        let extracted = extract(&text, doc_type(label));
        let topics = derive_topics(request, label, &extracted);
        let metadata = source_metadata(request, source, &sha, topics.clone())?;
        let spans: Vec<(usize, usize)> = extracted.iter().map(|x| (x.start, x.end)).collect();
        let chunking = chunk_source(&text, &spans, &topics, &run_id);
        if chunking.chunks.is_empty() {
            let reason = if chunking.truncated_at.is_some() {
                "too_large"
            } else {
                "unreadable"
            };
            exclude(&mut excluded, label, reason);
            continue;
        }
        if chunking.truncated_at.is_some() {
            // The stored prefix is kept; the unstorable tail is reported.
            exclude(&mut excluded, label, "too_large");
        }
        let mut written: Vec<((usize, usize), RecordRef)> = Vec::new();
        let chunk_count = chunking.chunks.len();
        for (start, end) in chunking.chunks {
            if !(ctx.alive)() {
                return Err(KnowledgeError::Locked);
            }
            if (ctx.elapsed_ms)() >= deadline {
                exhausted = true;
                stop = true;
                break;
            }
            let logical_id = format!("{run_id}-e{:03}", evidence_seq + 1);
            let content = &text[start..end];
            let terms = index_terms(content, &topics, &logical_id);
            if !load.admits(&terms) {
                exhausted = true;
                stop = true;
                break;
            }
            evidence_seq += 1;
            let record = KnowledgeRecord::Evidence(Evidence {
                header: header(
                    logical_id,
                    timestamp_ms,
                    metadata.clone(),
                    vec![],
                    reason.clone(),
                ),
                evidence_kind: EvidenceKind::Artifact,
                content: content.to_string(),
                content_sha256: knowledge::digest(content.as_bytes()),
            });
            let mapping = ctx.store.create(record).map_err(store_error)?;
            load.add(terms);
            evidence_refs.push(mapping.reference.clone());
            written.push(((start, end), mapping.reference));
        }
        if written.len() < chunk_count {
            // Deadline or index/catalog guard: the source is not (fully) stored.
            exclude(&mut excluded, label, "budget");
        }
        for item in &extracted {
            if stop {
                break;
            }
            let Some(((chunk_start, _), evidence)) = written
                .iter()
                .find(|((s, e), _)| *s <= item.start && item.end <= *e)
            else {
                continue;
            };
            let norm = normalize_statement(&item.statement);
            if norm.is_empty() || seen_statements.contains(&norm) {
                continue;
            }
            if candidates.len() >= request.budget.max_candidates {
                exhausted = true;
                stop = true;
                break;
            }
            if !(ctx.alive)() {
                return Err(KnowledgeError::Locked);
            }
            if (ctx.elapsed_ms)() >= deadline {
                exhausted = true;
                stop = true;
                break;
            }
            let logical_id = format!("{run_id}-c{:03}", candidate_seq + 1);
            let terms = index_terms(&item.statement, &topics, &logical_id);
            if !load.admits(&terms) {
                exhausted = true;
                stop = true;
                break;
            }
            let hits: Vec<(RecordRef, &'static str)> = existing_heads
                .iter()
                .filter_map(|(r, s)| {
                    contradiction_rule(&item.statement, s).map(|rule| (r.clone(), rule))
                })
                .take(MAX_CONTRADICTIONS_PER_CANDIDATE)
                .collect();
            let make = |edges: Vec<GraphEdge>| {
                KnowledgeRecord::Candidate(Candidate {
                    header: header(
                        logical_id.clone(),
                        timestamp_ms,
                        metadata.clone(),
                        edges,
                        reason.clone(),
                    ),
                    statement: item.statement.clone(),
                    evidence: vec![evidence.clone()],
                })
            };
            // Heuristic contradictions are REPORTED, never persisted as
            // Contradicting edges: under Stage 2/3 rules such an edge marks both
            // records contradictory permanently (append-only), so one lexical
            // false positive could block a verified pattern from current recall,
            // export and approval. The user decides from the report.
            let mapping = ctx.store.create(make(vec![])).map_err(store_error)?;
            candidate_seq += 1;
            seen_statements.insert(norm);
            load.add(terms);
            for (existing, rule) in hits {
                contradictions.push(ContradictionView {
                    candidate: mapping.reference.clone(),
                    existing,
                    rule: rule.into(),
                });
            }
            let excerpt = display_text(&text[item.start..item.end], MAX_EXCERPT_CHARS);
            candidates.push(DistilledCandidate {
                reference: mapping.reference,
                statement: item.statement.clone(),
                citation: Citation {
                    evidence: evidence.clone(),
                    start: item.start - chunk_start,
                    end: item.end - chunk_start,
                    excerpt,
                },
            });
        }
    }
    if !(ctx.alive)() {
        return Err(KnowledgeError::Locked);
    }
    let elapsed_ms = (ctx.elapsed_ms)();
    let body = RunSummaryBody {
        schema: RUN_SUMMARY_SCHEMA.into(),
        run_id: run_id.clone(),
        method: DISTILL_METHOD.into(),
        timestamp_ms,
        request_sha256: ctx.request_sha256.into(),
        evidence: evidence_refs.len(),
        candidates: candidates.len(),
        excluded: excluded
            .iter()
            .map(|x| SummaryExclusion {
                label_sha256: knowledge::digest(x.label.as_bytes())[..16].to_string(),
                reason: x.reason.clone(),
            })
            .collect(),
        possible_contradictions: contradictions.len(),
        budget_exhausted: exhausted,
        elapsed_ms,
    };
    let content = serde_json::to_string(&body).map_err(|_| KnowledgeError::Invalid)?;
    let summary_metadata = KnowledgeMetadata {
        source_id: format!("distill-run:{run_hex}"),
        source_version: RUN_SUMMARY_SCHEMA.into(),
        source_commit: ctx.request_sha256.into(),
        license: request.license.clone(),
        privacy: KnowledgePrivacy::Private,
        applicability: Applicability {
            topics: vec!["distill-run".into()],
            platforms: vec![request.platform.clone()],
            constraints: "distillation run summary: counts, method and exclusion reasons only; no source text".into(),
        },
    };
    let summary = ctx
        .store
        .create(KnowledgeRecord::Evidence(Evidence {
            header: header(
                format!("{run_id}-summary"),
                timestamp_ms,
                summary_metadata,
                vec![],
                reason,
            ),
            evidence_kind: EvidenceKind::Observation,
            content_sha256: knowledge::digest(content.as_bytes()),
            content,
        }))
        .map_err(store_error)?
        .reference;
    Ok(DistillReport {
        run_id,
        method: DISTILL_METHOD.into(),
        evidence: evidence_refs,
        candidates,
        excluded,
        possible_contradictions: contradictions,
        budget_exhausted: exhausted,
        elapsed_ms,
        summary,
    })
}

/// Newest first; only authenticated Observation evidence whose content is a
/// strict run-summary body bound to its own logical id.
pub(crate) fn run_summaries(snapshot: &Snapshot) -> Vec<DistillRunSummary> {
    let mut out = Vec::new();
    for mapping in snapshot.entries.iter().rev() {
        let Some(item) = snapshot.get(&mapping.reference) else {
            continue;
        };
        let KnowledgeRecord::Evidence(e) = &item.record else {
            continue;
        };
        if e.evidence_kind != EvidenceKind::Observation
            || !e.header.logical_id.starts_with("distill-")
            || !e.header.logical_id.ends_with("-summary")
        {
            continue;
        }
        let Ok(body) = serde_json::from_str::<RunSummaryBody>(&e.content) else {
            continue;
        };
        if body.schema != RUN_SUMMARY_SCHEMA
            || format!("{}-summary", body.run_id) != e.header.logical_id
        {
            continue;
        }
        out.push(DistillRunSummary {
            summary: mapping.reference.clone(),
            run_id: body.run_id,
            timestamp_ms: body.timestamp_ms,
            evidence: body.evidence,
            candidates: body.candidates,
            excluded: body.excluded.len(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span<'a>(text: &'a str, x: &Extracted) -> &'a str {
        &text[x.start..x.end]
    }

    #[test]
    fn knowledge_distiller_extraction_is_deterministic_with_exact_spans() {
        let md = "Intro line without heading.\n\n# Vault locking\n\nThe vault locks after five minutes. More text.\n\n## Empty heading\n## Recovery codes\n\n- Store codes offline.\n\n```\n# not a heading\n```\n### Fenced first\n\n```sh\nignored\n```\nParagraph without terminator\ncontinues here\n";
        let found = extract(md, DocType::Markdown);
        let statements: Vec<&str> = found.iter().map(|x| x.statement.as_str()).collect();
        assert_eq!(
            statements,
            [
                "Vault locking: The vault locks after five minutes.",
                "Recovery codes: Store codes offline.",
                "Fenced first: Paragraph without terminator continues here",
            ]
        );
        assert_eq!(
            span(md, &found[0]),
            "# Vault locking\n\nThe vault locks after five minutes."
        );
        assert_eq!(
            span(md, &found[1]),
            "## Recovery codes\n\n- Store codes offline."
        );
        assert!(span(md, &found[2]).starts_with("### Fenced first"));
        assert!(span(md, &found[2]).ends_with("continues here"));
        assert_eq!(found[0].anchor, "Vault locking");
        assert_eq!(found, extract(md, DocType::Markdown), "deterministic");

        let py = "\"\"\"Module doc.\"\"\"\nimport math\n\n\ndef add(a, b):\n    \"\"\"Return the sum.\"\"\"\n    return a + b\n\n\nclass Calc(\n    Base,\n):  # comment\n    \"\"\"\n    Stateful calculator.\n    \"\"\"\n\n    def inner(self):\n        \"\"\"Not top level.\"\"\"\n\nasync def fetch(url):\n    return url\n";
        let found = extract(py, DocType::Python);
        let statements: Vec<&str> = found.iter().map(|x| x.statement.as_str()).collect();
        assert_eq!(
            statements,
            [
                "def add(a, b): Return the sum.",
                "class Calc( Base, ): Stateful calculator.",
                "async def fetch(url)",
            ]
        );
        assert_eq!(
            span(py, &found[0]),
            "def add(a, b):\n    \"\"\"Return the sum.\"\"\""
        );
        assert!(span(py, &found[1]).ends_with("Stateful calculator."));
        assert_eq!(found[2].anchor, "fetch");

        let toml = "\n[package]\nname = \"x\"\nversion = \"1\"\nedition = \"2021\"\n";
        let found = extract(toml, DocType::Other);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].statement, "[package] name = \"x\" version = \"1\"");
        assert_eq!(
            span(toml, &found[0]),
            "[package]\nname = \"x\"\nversion = \"1\""
        );
        // Headingless text falls back to the first lines.
        let plain = extract("alpha\nbeta\n", DocType::Markdown);
        assert_eq!(plain[0].statement, "alpha beta");

        assert_eq!(doc_type("notes/guide.MD"), DocType::Markdown);
        assert_eq!(doc_type("README"), DocType::Markdown);
        assert_eq!(doc_type("src/calc.py"), DocType::Python);
        assert_eq!(doc_type("Cargo.toml"), DocType::Other);
        assert_eq!(extension(".bashrc"), None);
        // Untrusted display text: controls/bidi become spaces, never merge terms.
        assert_eq!(display_text("a\u{202E}b\u{0007}c\n\n d", 280), "a b c d");
        assert_eq!(display_text("abcdef", 3), "abc");
        assert_eq!(display_text("ab cd", 3), "ab");
        assert_eq!(normalize_statement("Hello,  World!"), "hello world");
    }

    #[test]
    fn knowledge_distiller_chunks_respect_stage3_term_caps() {
        let topics = vec!["security".to_string(), "md".to_string()];
        let id = "distill-0123456789abcdef0123456789abcdef";
        let mut text = String::new();
        let mut spans = Vec::new();
        for k in 0..8 {
            let start = text.len();
            text.push_str(&format!("# Part {k}\n\nPart {k} explains group {k}.\n"));
            spans.push((start, text.len() - 1));
            text.push('\n');
            for line in 0..20 {
                let words: Vec<String> = (0..10)
                    .map(|w| format!("w{}x", k * 1000 + line * 10 + w))
                    .collect();
                text.push_str(&words.join(" "));
                text.push('\n');
            }
        }
        let chunking = chunk_source(&text, &spans, &topics, id);
        assert_eq!(chunking.truncated_at, None);
        assert!(chunking.chunks.len() >= 3, "{:?}", chunking.chunks);
        let mut previous_end = 0;
        for (n, (s, e)) in chunking.chunks.iter().enumerate() {
            assert_eq!(*s, previous_end, "contiguous");
            previous_end = *e;
            let logical = format!("{id}-e{:03}", n + 1);
            let terms = index_terms(&text[*s..*e], &topics, &logical);
            assert!(terms.len() <= INDEX_MAX_DOCUMENT_TERMS, "{}", terms.len());
            assert!(e - s <= MAX_CHUNK_BYTES);
            for (a, b) in &spans {
                assert!(!(a < s && s < b), "cut inside a span");
            }
        }
        assert_eq!(previous_end, text.len());

        // A term over the Stage 3 term cap truncates the source at its line.
        let long = format!("# Ok\n\nFine.\n{}\nafter\n", "z".repeat(300));
        let chunking = chunk_source(&long, &[], &topics, id);
        assert_eq!(chunking.truncated_at, Some("# Ok\n\nFine.\n".len()));
        assert_eq!(chunking.chunks, vec![(0, "# Ok\n\nFine.\n".len())]);
        // One huge line of distinct words is split at whitespace.
        let line: Vec<String> = (0..2000).map(|w| format!("q{w}")).collect();
        let line = line.join(" ");
        let chunking = chunk_source(&line, &[], &topics, id);
        assert_eq!(chunking.truncated_at, None);
        assert!(chunking.chunks.len() >= 4);
        for (s, e) in &chunking.chunks {
            assert!(index_terms(&line[*s..*e], &topics, id).len() < INDEX_MAX_DOCUMENT_TERMS);
        }
        // Mirror of the Stage 3 tokenizer.
        assert_eq!(
            lexical_terms("Vault-Lock pai:index my_id x.y"),
            ["vault", "lock", "pai:index", "my_id", "x", "y"]
        );
    }

    #[test]
    fn knowledge_distiller_contradiction_heuristic_rules() {
        let existing = "Cache eviction: The cache eviction policy runs during backup windows.";
        assert_eq!(
            contradiction_rule(
                "Cache eviction: The cache eviction policy must never run during backup windows.",
                existing
            ),
            Some(RULE_NEGATION)
        );
        assert_eq!(
            contradiction_rule(
                "Disable cache eviction policy for backups.",
                "Cache eviction policy for backups."
            ),
            Some(RULE_NEGATION)
        );
        assert_eq!(
            contradiction_rule(
                "Cache eviction policy does not run during backup windows.",
                "Cache eviction policy will never run during backup windows."
            ),
            None,
            "both negated"
        );
        assert_eq!(
            contradiction_rule(
                "Cache eviction is not allowed.",
                "Backups use separate disks."
            ),
            None,
            "fewer than three shared terms"
        );
        assert_eq!(
            contradiction_rule(
                "Session timeout policy for vault unlock is 30 minutes.",
                "Session timeout policy for vault unlock is 60 minutes."
            ),
            Some(RULE_NUMBER)
        );
        assert_eq!(
            contradiction_rule(
                "Session timeout policy for vault unlock is 30 minutes.",
                "Session timeout policy for vault unlock is 30 minutes total."
            ),
            None
        );
    }

    fn request(sources: Vec<DistillSource>) -> DistillRequest {
        DistillRequest {
            sources,
            budget: DistillBudget {
                max_total_bytes: 4096,
                max_candidates: 8,
                deadline_ms: 1000,
            },
            platform: "linux-x86_64".into(),
            license: "unknown".into(),
            topics: vec!["fixture".into()],
        }
    }

    #[test]
    fn knowledge_distiller_request_validation_hash_and_serde_shapes() {
        let pasted = || DistillSource::PastedText {
            label: "notes.md".into(),
            text: "# A\n\nB.\n".into(),
        };
        let valid = request(vec![pasted()]);
        let hash = distill_request_sha256(&valid).unwrap();
        assert_eq!(hash, distill_request_sha256(&valid.clone()).unwrap());
        assert_eq!(
            hash,
            knowledge::digest(&serde_json::to_vec(&valid).unwrap()),
            "sha256 of canonical serde_json"
        );
        let mut changed = valid.clone();
        changed.license = "MIT".into();
        assert_ne!(hash, distill_request_sha256(&changed).unwrap());
        let mut bad = Vec::new();
        bad.push(request(vec![]));
        bad.push(request((0..33).map(|_| pasted()).collect()));
        for (bytes, candidates, deadline) in [
            (0, 8, 1000),
            (MAX_TOTAL_BYTES + 1, 8, 1000),
            (4096, 0, 1000),
            (4096, 65, 1000),
            (4096, 8, 0),
            (4096, 8, 30_001),
        ] {
            let mut r = valid.clone();
            r.budget = DistillBudget {
                max_total_bytes: bytes,
                max_candidates: candidates,
                deadline_ms: deadline,
            };
            bad.push(r);
        }
        let mut r = valid.clone();
        r.license = "x".repeat(257);
        bad.push(r);
        let mut r = valid.clone();
        r.platform = "linux\u{7}".into();
        bad.push(r);
        let mut r = valid.clone();
        r.topics = (0..9).map(|i| format!("t{i}")).collect();
        bad.push(r);
        let mut r = valid.clone();
        r.topics = vec!["dup".into(), "dup".into()];
        bad.push(r);
        bad.push(request(vec![DistillSource::PastedText {
            label: "x".repeat(129),
            text: "a".into(),
        }]));
        bad.push(request(vec![DistillSource::PastedText {
            label: "blank".into(),
            text: " \n ".into(),
        }]));
        bad.push(request(vec![DistillSource::LocalFile {
            root: PathBuf::from("relative/root"),
            path: "a.md".into(),
        }]));
        for r in &bad {
            assert_eq!(
                distill_request_sha256(r),
                Err(KnowledgeError::Invalid),
                "{r:?}"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let root = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff\xfe/root"));
            let r = request(vec![DistillSource::LocalFile {
                root,
                path: "a.md".into(),
            }]);
            assert_eq!(distill_request_sha256(&r), Err(KnowledgeError::Invalid));
        }
        // Wire shapes the UI is built against.
        let json = serde_json::to_value(&valid).unwrap();
        assert_eq!(json["sources"][0]["kind"], "pasted_text");
        assert_eq!(json["budget"]["max_total_bytes"], 4096);
        let local: DistillSource = serde_json::from_value(serde_json::json!({
            "kind": "local_file", "root": "/tmp/project", "path": "src/a.py"
        }))
        .unwrap();
        assert!(matches!(local, DistillSource::LocalFile { .. }));
        for hostile in [
            serde_json::json!({"kind": "pasted_text", "label": "a", "text": "b", "extra": 1}),
            serde_json::json!({"kind": "shell", "command": "rm"}),
        ] {
            assert!(serde_json::from_value::<DistillSource>(hostile).is_err());
        }
        let mut json = serde_json::to_value(&valid).unwrap();
        json["budget"]["unexpected"] = true.into();
        assert!(serde_json::from_value::<DistillRequest>(json).is_err());
        let mut json = serde_json::to_value(&valid).unwrap();
        json["model"] = "cloud".into();
        assert!(serde_json::from_value::<DistillRequest>(json).is_err());
        assert!(serde_json::from_value::<UiDistillEvent>(
            serde_json::json!({"request_sha256": hash, "ui_event_id": "a", "auto": true})
        )
        .is_err());
        assert_eq!(
            DISTILL_METHOD,
            "extractive-headings-docstrings-v1; deterministic; no model; no network"
        );
    }

    #[test]
    fn knowledge_distiller_index_guard_refuses_unbuildable_growth() {
        let terms = |n: usize| -> BTreeSet<String> { (0..n).map(|i| format!("t{i}")).collect() };
        let load = IndexLoad {
            postings: 0,
            terms: BTreeSet::new(),
            entries: 0,
        };
        assert!(load.admits(&terms(INDEX_MAX_DOCUMENT_TERMS)));
        assert!(!load.admits(&terms(INDEX_MAX_DOCUMENT_TERMS + 1)));
        assert!(!load.admits(&["x".repeat(INDEX_MAX_TERM_CHARS + 1)].into()));
        let full = IndexLoad {
            postings: INDEX_GUARD_POSTINGS - 10,
            terms: BTreeSet::new(),
            entries: 0,
        };
        assert!(full.admits(&terms(10)) && !full.admits(&terms(11)));
        let distinct = IndexLoad {
            postings: 0,
            terms: terms(INDEX_GUARD_TERMS),
            entries: 0,
        };
        assert!(
            distinct.admits(&terms(5)),
            "already-known terms add no new terms"
        );
        assert!(!distinct.admits(&["fresh-term".to_string()].into()));
        let catalog = IndexLoad {
            postings: 0,
            terms: BTreeSet::new(),
            entries: CATALOG_GUARD - 2,
        };
        assert!(catalog.admits(&terms(1)));
        let catalog = IndexLoad {
            entries: CATALOG_GUARD - 1,
            ..catalog
        };
        assert!(!catalog.admits(&terms(1)));
    }

    /// Windows/non-Linux: no fd-relative capture, LocalFile is never read.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn knowledge_distiller_local_file_unreadable_off_linux() {
        let config = KnowledgeServiceConfig {
            scratch: std::env::temp_dir(),
            excluded_sha256: BTreeSet::new(),
            excluded_name_markers: vec![],
        };
        assert!(read_local(&config, std::path::Path::new("C:\\granted"), "notes.md").is_err());
    }
}
