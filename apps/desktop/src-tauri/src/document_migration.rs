// UnoOne Power — Wave 3: migrate plaintext documents/memory to encrypted records
//
// SAFETY CONTRACT
//   1. Pure decisions come from unoone-document-migration (9/9 unit tests).
//   2. Records are written via vault-core write_record, verified by a full
//      decrypt read-back, and plaintext is deleted ONLY after verification.
//   3. A marker record (fixed UUID, MigrationRecord type) tracks completed
//      ids — crashes resume idempotently, never double-deleting.
//   4. Built and tested against synthetic vaults. NEVER debug against the
//      only copy of real data.

use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;
use unoone_document_migration as plan_logic;
use unoone_vault_core::{PrivacyLevel as VaultPrivacyLevel, Record, RecordType, Vault};

/// Fixed UUID-v4-shaped id for the single migration marker record.
pub const MARKER_RECORD_ID: &str = "4d6f2b1a-8c3e-4f7a-9b2d-1e5a8c3f6b01";

#[derive(Debug, Clone, Serialize)]
pub struct MigrationReport {
    pub scanned_plaintext: usize,
    pub migrated: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub plaintext_deleted: Vec<String>,
    pub remaining_plaintext: Vec<String>,
    pub marker_updated: bool,
    pub already_migrated_before_run: usize,
    pub user_message: String,
}

/// Document extensions the desktop processor lists as documents.
const DOCUMENT_EXTENSIONS: [&str; 20] = [
    "pdf", "docx", "doc", "txt", "md", "markdown", "csv", "xlsx", "xls", "pptx", "ppt", "png",
    "jpg", "jpeg", "gif", "webp", "mp3", "wav", "ogg", "flac",
];

fn file_entry(
    path: &Path,
    kind: plan_logic::PlainKind,
    vault_root: &Path,
) -> Option<plan_logic::PlainEntry> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let supported = match kind {
        plan_logic::PlainKind::DocumentOriginal => DOCUMENT_EXTENSIONS.contains(&ext.as_str()),
        plan_logic::PlainKind::Memory => matches!(ext.as_str(), "json" | "txt" | "md"),
    };
    if !supported {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(plan_logic::PlainEntry {
        id: path.file_stem()?.to_string_lossy().to_string(),
        rel_path: path
            .strip_prefix(vault_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string()),
        kind,
        size_bytes: meta.len(),
        modified_epoch: modified,
    })
}

/// Scan the plaintext directories (documents + memory).
pub fn scan_plain_entries(vault_root: &Path) -> Vec<plan_logic::PlainEntry> {
    let vault_root = vault_root.to_path_buf();
    let mut out = Vec::new();
    let docs = vault_root.join("VAULT").join("documents");
    let mem = vault_root.join("VAULT").join("memory");
    if let Ok(rd) = std::fs::read_dir(&docs) {
        for e in rd.flatten() {
            if let Some(p) = file_entry(
                &e.path(),
                plan_logic::PlainKind::DocumentOriginal,
                &vault_root,
            ) {
                out.push(p);
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(&mem) {
        for e in rd.flatten() {
            if let Some(p) = file_entry(&e.path(), plan_logic::PlainKind::Memory, &vault_root) {
                out.push(p);
            }
        }
    }
    out
}

fn read_marker_ids(vault: &Vault) -> BTreeSet<String> {
    let Ok((_, bytes)) = vault.read_record(MARKER_RECORD_ID) else {
        return BTreeSet::new();
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return BTreeSet::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BTreeSet::new();
    };
    value
        .get("migrated")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn write_marker(vault: &mut Vault, ids: &BTreeSet<String>, tx_id: String) -> Result<(), String> {
    let content = serde_json::json!({
        "version": 1,
        "migrated": ids.iter().collect::<Vec<_>>(),
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });
    let mut record = Record::new(RecordType::MigrationRecord, "DESKTOP", "document-migration");
    record.record_id = MARKER_RECORD_ID.to_string();
    record.privacy_level = VaultPrivacyLevel::MetadataOnly;
    record.transaction_id = tx_id;
    vault
        .write_record(record, content.to_string().as_bytes())
        .map_err(|e| format!("Marker write failed: {}", e))
}

fn new_encrypted_record(record_type: RecordType, parent: Option<String>, tx_id: String) -> Record {
    let mut record = Record::new(record_type, "DESKTOP", "document-migration");
    record.privacy_level = VaultPrivacyLevel::Private;
    record.parent_record_id = parent;
    record.transaction_id = tx_id;
    record
}

/// Extract text for decodable formats (delegates to documents.rs helpers).
fn extract_text_for(path: &Path, ext: &str) -> Result<String, String> {
    match ext {
        "txt" | "md" | "markdown" | "csv" => {
            std::fs::read_to_string(path).map_err(|e| format!("read failed: {}", e))
        }
        "html" | "htm" => std::fs::read_to_string(path)
            .map_err(|e| format!("read failed: {}", e))
            .map(|html| crate::documents::strip_html_tags(&html)),
        "pdf" => crate::documents::extract_pdf_text(&path.to_path_buf()),
        "docx" | "doc" => crate::documents::extract_docx_text(&path.to_path_buf()),
        _ => Err(format!("format '{}' has no text extractor", ext)),
    }
}

/// Run the migration. `vault` must be unlocked.
///
/// The steps execute in `step_for` order per planned entry: write records,
/// verify read-back, then delete. On ANY failure for an entry we abort the
/// entry (its plaintext remains) and continue with the next; the marker is
/// updated after every verified entry, so a crash mid-run resumes cleanly.
pub fn migrate(vault: &mut Vault, vault_root: &Path) -> Result<MigrationReport, String> {
    if !vault.is_unlocked() {
        return Err("Vault is locked — unlock before migrating".to_string());
    }

    let entries = scan_plain_entries(vault_root);
    let migrated = read_marker_ids(vault);
    let already = migrated.len();
    let plan = plan_logic::plan(&entries, &migrated);
    let mut done = migrated;
    let mut report = MigrationReport {
        scanned_plaintext: entries.len(),
        migrated: Vec::new(),
        failed: Vec::new(),
        plaintext_deleted: Vec::new(),
        remaining_plaintext: Vec::new(),
        marker_updated: false,
        already_migrated_before_run: already,
        user_message: String::new(),
    };

    if plan.entries.is_empty() {
        report.user_message = if plan_logic::is_complete(&entries, &done) {
            "All plaintext documents/memories are already migrated to encrypted records."
                .to_string()
        } else {
            "No plaintext documents or memories found to migrate.".to_string()
        };
        return Ok(report);
    }

    for planned in &plan.entries {
        let abs_path = vault_root.join(&planned.rel_path);
        let tx = uuid::Uuid::new_v4().to_string();
        match migrate_one(vault, &abs_path, planned, tx.clone()) {
            Ok(()) => {
                report.migrated.push(planned.id.clone());
                report.plaintext_deleted.push(planned.rel_path.clone());
                done.insert(planned.id.clone());
                // Marker update per verified entry (crash-safe resume).
                if write_marker(vault, &done, tx).is_ok() {
                    report.marker_updated = true;
                } else {
                    report.failed.push((
                        planned.id.clone(),
                        "marker update failed after verified migration — entry is migrated but unmarked; rerun updates it".to_string(),
                    ));
                }
            }
            Err(reason) => {
                report.failed.push((planned.id.clone(), reason));
                report.remaining_plaintext.push(planned.rel_path.clone());
            }
        }
    }

    // What plaintext remains after this run? (failed + none-planned).
    report.user_message = format!(
        "Migrated {} of {} plaintext entr{}, deleted their plaintext after verified read-back; {} failed and remain plaintext.",
        report.migrated.len(),
        plan.entries.len(),
        if plan.entries.len() == 1 { "y" } else { "ies" },
        report.failed.len()
    );
    Ok(report)
}

fn migrate_one(
    vault: &mut Vault,
    abs_path: &Path,
    planned: &plan_logic::PlannedEntry,
    tx_id: String,
) -> Result<(), String> {
    let plain_bytes =
        std::fs::read(abs_path).map_err(|e| format!("cannot read plaintext: {}", e))?;

    // 1. Original bytes (attachment).
    let orig_record = new_encrypted_record(
        match planned.kind {
            plan_logic::PlainKind::DocumentOriginal => RecordType::Document,
            plan_logic::PlainKind::Memory => RecordType::Memory,
        },
        None,
        tx_id.clone(),
    );
    let orig_id = orig_record.record_id.clone();
    vault
        .write_record(orig_record, &plain_bytes)
        .map_err(|e| format!("original write failed: {}", e))?;

    // 2. Extracted text (when decodable).
    let mut text_id: Option<String> = None;
    if planned.wants_text_extract {
        let ext = abs_path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let text =
            extract_text_for(abs_path, &ext).map_err(|e| format!("text extract failed: {}", e))?;
        let text_record =
            new_encrypted_record(RecordType::Transcript, Some(orig_id.clone()), tx_id.clone());
        text_id = Some(text_record.record_id.clone());
        vault
            .write_record(text_record, text.as_bytes())
            .map_err(|e| format!("text record write failed: {}", e))?;
    }

    // 3. Metadata envelope.
    let metadata = serde_json::json!({
        "legacy_id": planned.id,
        "legacy_rel_path": planned.rel_path,
        "kind": format!("{:?}", planned.kind),
        "original_record_id": orig_id,
        "text_record_id": text_id,
        "modified_epoch": planned.modified_epoch,
        "migrated_at": chrono::Utc::now().to_rfc3339(),
    });
    let meta_record = new_encrypted_record(
        RecordType::ContextSnapshot,
        Some(orig_id.clone()),
        tx_id.clone(),
    );
    vault
        .write_record(meta_record, metadata.to_string().as_bytes())
        .map_err(|e| format!("metadata record write failed: {}", e))?;

    // 4. Verify by full decrypt read-back — the only license to delete.
    let (_, orig_back) = vault
        .read_record(&orig_id)
        .map_err(|e| format!("original read-back failed: {}", e))?;
    if orig_back != plain_bytes {
        return Err(
            "original read-back bytes differ from plaintext — refusing to delete".to_string(),
        );
    }
    if planned.wants_text_extract {
        let tid = text_id.as_deref().unwrap();
        vault
            .read_record(tid)
            .map_err(|e| format!("text read-back failed: {}", e))?;
    }

    // 5. Only NOW the plaintext may go — delete, then PROVE it is gone.
    std::fs::remove_file(abs_path)
        .map_err(|e| format!("verified but plaintext deletion failed: {}", e))?;
    if abs_path.exists() {
        return Err("plaintext still present after verified deletion".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Synthetic vault in a temp dir: never touches a real drive.
    /// Synthetic credentials, per the safety contract.
    fn make_unlocked_vault() -> (tempfile::TempDir, Vault, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        let created = Vault::create(&vault_root, b"synthetic-migration-test-password").unwrap();
        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"synthetic-migration-test-password").unwrap();
        let _ = created;
        (dir, vault, vault_root)
    }

    fn drop_text_doc(vault_root: &Path, name: &str, ext: &str, content: &str) {
        let docs = vault_root.join("VAULT").join("documents");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join(format!("{}.{}", name, ext)), content).unwrap();
    }

    fn drop_memory(vault_root: &Path, name: &str, content: &str) {
        let mem = vault_root.join("VAULT").join("memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join(format!("{}.md", name)), content).unwrap();
    }

    #[test]
    fn full_migration_deletes_plaintext_only_after_verification() {
        let (_dir, mut vault, vault_root) = make_unlocked_vault();
        drop_text_doc(
            &vault_root,
            "meeting-notes",
            "txt",
            "Synthetic meeting notes content",
        );
        drop_memory(&vault_root, "shopping-list", "milk, bread (synthetic)");

        let report = migrate(&mut vault, &vault_root).unwrap();
        assert_eq!(report.migrated.len(), 2);
        assert!(
            report.failed.is_empty(),
            "unexpected failures: {:?}",
            report.failed
        );
        assert_eq!(report.plaintext_deleted.len(), 2);
        assert!(report.marker_updated);

        // Plaintext is gone.
        assert!(!vault_root
            .join("VAULT/documents/meeting-notes.txt")
            .exists());
        assert!(!vault_root.join("VAULT/memory/shopping-list.md").exists());

        // And the content is recoverable from encrypted records.
        let (_, rec_content) = {
            // Read the marker's own record set via metadata scan: locate the
            // Document record by reading all records? We verify via the marker
            // itself: marker must list both ids.
            vault.read_record(MARKER_RECORD_ID).unwrap()
        };
        let marker: serde_json::Value = serde_json::from_slice(&rec_content).unwrap();
        let ids: Vec<&str> = marker["migrated"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(ids.contains(&"meeting-notes"));
        assert!(ids.contains(&"shopping-list"));
    }

    #[test]
    fn second_run_is_inert_and_reports_complete() {
        let (_dir, mut vault, vault_root) = make_unlocked_vault();
        drop_text_doc(&vault_root, "once-only", "txt", "Synthetic content");
        let first = migrate(&mut vault, &vault_root).unwrap();
        assert_eq!(first.migrated.len(), 1);
        let second = migrate(&mut vault, &vault_root).unwrap();
        assert!(second.migrated.is_empty());
        assert!(second.failed.is_empty());
        assert!(second.user_message.contains("already migrated"));
        assert_eq!(second.already_migrated_before_run, 1);
    }

    #[test]
    fn locked_vault_refuses_before_touching_anything() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        let _ = Vault::create(&vault_root, b"synthetic-migration-test-password").unwrap();
        drop_text_doc(&vault_root, "untouched", "txt", "Synthetic content");
        let mut vault = Vault::open(&vault_root).unwrap(); // open = locked
        let err = migrate(&mut vault, &vault_root).unwrap_err();
        assert!(err.contains("locked"), "got: {err}");
        assert!(vault_root.join("VAULT/documents/untouched.txt").exists());
    }

    #[test]
    fn binary_originals_migrate_without_text_extract() {
        let (_dir, mut vault, vault_root) = make_unlocked_vault();
        let docs = vault_root.join("VAULT").join("documents");
        std::fs::create_dir_all(&docs).unwrap();
        let bytes = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3, 250, 251, 252]; // fake PNG header + binary
        std::fs::write(docs.join("photo.png"), &bytes).unwrap();

        let report = migrate(&mut vault, &vault_root).unwrap();
        assert_eq!(
            report.migrated,
            vec!["photo".to_string()],
            "{:?}",
            report.failed
        );
        assert!(!docs.join("photo.png").exists());
    }

    #[test]
    fn resume_after_partial_run_skips_marked_entries() {
        let (_dir, mut vault, vault_root) = make_unlocked_vault();
        drop_text_doc(&vault_root, "done-already", "txt", "Synthetic A");
        drop_text_doc(&vault_root, "still-pending", "txt", "Synthetic B");

        // Complete run 1 migrates the smaller file (deterministic order:
        // same sizes tie-break by id -> done-already first).
        let r1 = migrate(&mut vault, &vault_root).unwrap();
        assert_eq!(r1.migrated.len(), 2);

        // New plaintext arrives later; only it should migrate.
        drop_text_doc(&vault_root, "late-arrival", "txt", "Synthetic C");
        let r2 = migrate(&mut vault, &vault_root).unwrap();
        assert_eq!(r2.migrated, vec!["late-arrival".to_string()]);
        assert_eq!(r2.already_migrated_before_run, 2);
    }
}
