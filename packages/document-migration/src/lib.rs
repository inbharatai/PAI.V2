//! unoone-document-migration — pure decision logic for Wave 3 (plaintext →
//! encrypted vault records).
//!
//! This crate knows NOTHING about vault-core, encryption, or the filesystem.
//! It answers four questions deterministically from scan inputs:
//!
//!  1. Which plaintext entries still need migrating? (idempotence)
//!  2. In what order? (deterministic — same input, same plan)
//!  3. What steps must happen per entry? (write records → verify → delete)
//!  4. Is a prior run's partial state dangerous? (never delete what was not
//!     verified inside the vault)
//!
//! The thin adapter in apps/desktop/src-tauri feeds it real directory scans
//! and executes the steps against vault-core. Everything that decides safety
//! lives here, where any host can run the tests.

use std::collections::BTreeSet;

/// Kind of plaintext artifact found on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlainKind {
    DocumentOriginal,
    Memory,
}

/// One plaintext artifact observed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainEntry {
    /// Stable identifier (file stem, as documents.rs already uses).
    pub id: String,
    /// Path relative to the vault root — only used for reporting/ordering.
    pub rel_path: String,
    pub kind: PlainKind,
    pub size_bytes: u64,
    pub modified_epoch: u64,
}

/// A single ordered step for one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Write the original bytes as an encrypted document/attachment record.
    WriteOriginal,
    /// Write the extracted text as an encrypted record linked to the original.
    WriteExtractedText,
    /// Write the metadata envelope as an encrypted record.
    WriteMetadata,
    /// Re-read every written record and confirm content bytes match. WITHOUT
    /// this succeeding the plaintext must never be deleted.
    VerifyReadback,
    /// Delete the plaintext artifact (allowed ONLY after VerifyReadback).
    DeletePlaintext,
}

/// Ordered work list for one plaintext entry.
#[derive(Debug, Clone)]
pub struct PlannedEntry {
    pub id: String,
    pub rel_path: String,
    pub kind: PlainKind,
    /// Extracted-text record is only produced when the format is textually
    /// decodable; binary or undecodable originals are preserved whole.
    pub wants_text_extract: bool,
    /// File modification time carried for the metadata record.
    pub modified_epoch: u64,
}

/// The migration plan: an ordered list of entries plus the mandatory marker.
#[derive(Debug, Default)]
pub struct Plan {
    pub entries: Vec<PlannedEntry>,
    /// True when a marker update must be written (any changes happened, or a
    /// marker never existed — an unmarked migrated state is indistinguishable
    /// from an unmigrated one and must not be relied upon).
    pub write_marker: bool,
}

/// Formats the desktop processor can decode to text today.
/// Keep in sync with documents.rs (TXT/MD/CSV/HTML fully; PDF basic; DOCX basic).
pub fn format_is_text_decodable(extension_lower: &str) -> bool {
    matches!(
        extension_lower,
        "txt" | "md" | "markdown" | "csv" | "html" | "htm" | "pdf" | "docx" | "doc"
    )
}

/// The canonical step sequence for one entry. Verification strictly precedes
/// deletion — this order is the entire safety argument.
pub fn steps_for(wants_text_extract: bool) -> &'static [Step] {
    if wants_text_extract {
        &[
            Step::WriteOriginal,
            Step::WriteExtractedText,
            Step::WriteMetadata,
            Step::VerifyReadback,
            Step::DeletePlaintext,
        ]
    } else {
        &[
            Step::WriteOriginal,
            Step::WriteMetadata,
            Step::VerifyReadback,
            Step::DeletePlaintext,
        ]
    }
}

/// Which ids may be deleted on THIS run? Only ids recorded as fully migrated
/// by a PREVIOUS marker update whose steps all completed. Never the ones we
/// are about to write (they must pass VerifyReadback first).
pub fn deletable_ids(migrated_ids: &BTreeSet<String>, plan: &Plan) -> BTreeSet<String> {
    let planned: BTreeSet<String> = plan.entries.iter().map(|e| e.id.clone()).collect();
    migrated_ids
        .iter()
        .filter(|id| !planned.contains(*id))
        .cloned()
        .collect()
}

/// Decide what still needs doing.
///
/// - `entries`  : everything found in plaintext directories.
/// - `migrated` : ids listed by the previous migration marker.
/// - `decodable_extensions` : which extensions yield extracted text
///   (passed in full, lowercase, so the desktop adapter stays in sync).
pub fn plan(entries: &[PlainEntry], migrated: &BTreeSet<String>) -> Plan {
    let mut remaining: Vec<PlannedEntry> = entries
        .iter()
        .filter(|e| !migrated.contains(&e.id))
        .map(|e| {
            let ext = e.rel_path.rsplit('.').next().unwrap_or("").to_lowercase();
            PlannedEntry {
                id: e.id.clone(),
                rel_path: e.rel_path.clone(),
                kind: e.kind,
                wants_text_extract: format_is_text_decodable(&ext),
                modified_epoch: e.modified_epoch,
            }
        })
        .collect();
    // Deterministic order: smallest first (fast progress, bounded failure
    // blast radius), then id for ties.
    remaining.sort_by(|a, b| {
        let sa = entries
            .iter()
            .find(|e| e.id == a.id)
            .map(|e| e.size_bytes)
            .unwrap_or(0);
        let sb = entries
            .iter()
            .find(|e| e.id == b.id)
            .map(|e| e.size_bytes)
            .unwrap_or(0);
        sa.cmp(&sb).then(a.id.cmp(&b.id))
    });
    Plan {
        write_marker: !remaining.is_empty(),
        entries: remaining,
    }
}

/// A plan is exhausted and only deletion of previously-marked ids remains —
/// meaning everything is already migrated: nothing to do, marker stays as is.
pub fn is_complete(entries: &[PlainEntry], migrated: &BTreeSet<String>) -> bool {
    entries.iter().all(|e| migrated.contains(&e.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, ext: &str, size: u64, kind: PlainKind) -> PlainEntry {
        PlainEntry {
            id: id.to_string(),
            rel_path: format!("VAULT/documents/{}.{}", id, ext),
            kind,
            size_bytes: size,
            modified_epoch: 1,
        }
    }

    #[test]
    fn plan_excludes_already_migrated() {
        let entries = vec![
            entry("a", "txt", 10, PlainKind::DocumentOriginal),
            entry("b", "txt", 20, PlainKind::DocumentOriginal),
        ];
        let migrated: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        let p = plan(&entries, &migrated);
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].id, "b");
    }

    #[test]
    fn plan_orders_smallest_first_then_id() {
        let entries = vec![
            entry("z", "txt", 50, PlainKind::DocumentOriginal),
            entry("y", "txt", 10, PlainKind::DocumentOriginal),
            entry("x", "txt", 10, PlainKind::DocumentOriginal),
        ];
        let p = plan(&entries, &BTreeSet::new());
        let order: Vec<&str> = p.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(order, vec!["x", "y", "z"]);
    }

    #[test]
    fn binary_original_never_gets_text_extract_step() {
        let entries = vec![entry("img", "png", 999, PlainKind::DocumentOriginal)];
        let p = plan(&entries, &BTreeSet::new());
        assert!(!p.entries[0].wants_text_extract);
        let steps = steps_for(p.entries[0].wants_text_extract);
        assert!(!steps.contains(&Step::WriteExtractedText));
    }

    #[test]
    fn verification_strictly_precedes_deletion() {
        for &wants in &[true, false] {
            let steps = steps_for(wants);
            let verify_at = steps
                .iter()
                .position(|s| *s == Step::VerifyReadback)
                .unwrap();
            let delete_at = steps
                .iter()
                .position(|s| *s == Step::DeletePlaintext)
                .unwrap();
            assert!(
                verify_at < delete_at,
                "deletion must never precede verification"
            );
        }
    }

    #[test]
    fn marker_written_whenever_work_remains_or_never_existed() {
        let entries = vec![entry("a", "txt", 10, PlainKind::DocumentOriginal)];
        assert!(plan(&entries, &BTreeSet::new()).write_marker);
        // Marker exists and everything done -> no rewrite needed.
        let migrated: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        assert!(!plan(&entries, &migrated).write_marker);
    }

    #[test]
    fn deletable_ids_never_include_pending_entries() {
        let entries = vec![entry("a", "txt", 10, PlainKind::DocumentOriginal)];
        let p = plan(&entries, &BTreeSet::new());
        let migrated: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        // 'a' is in BOTH the marker and the new plan -> it must not be deleted
        // on this run; it has to re-verify first.
        let del = deletable_ids(&migrated, &p);
        assert!(!del.contains("a"));
    }

    #[test]
    fn deletable_ids_are_only_previously_marked() {
        let p = Plan::default();
        let migrated: BTreeSet<String> = ["old-a".to_string(), "old-b".to_string()]
            .into_iter()
            .collect();
        let del = deletable_ids(&migrated, &p);
        assert_eq!(del, migrated);
    }

    #[test]
    fn completion_requires_every_entry_marked() {
        let entries = vec![
            entry("a", "txt", 1, PlainKind::DocumentOriginal),
            entry("b", "md", 2, PlainKind::Memory),
        ];
        let one_done: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        assert!(!is_complete(&entries, &one_done));
        let all_done: BTreeSet<String> = ["a".to_string(), "b".to_string()].into_iter().collect();
        assert!(is_complete(&entries, &all_done));
    }

    #[test]
    fn text_decodable_matrix() {
        for ext in ["txt", "md", "csv", "html", "pdf", "docx"] {
            assert!(format_is_text_decodable(ext), "{ext} should decode");
        }
        for ext in ["png", "mp3", "xlsx", "bin", ""] {
            assert!(!format_is_text_decodable(ext), "{ext} should not decode");
        }
    }
}
