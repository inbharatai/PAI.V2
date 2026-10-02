// Persistent conversation memory — the chat history the user asks for:
// every completed turn pair is written to the encrypted vault as a MESSAGE
// record, and a new session recalls the most recent turns so the model
// starts with continuity instead of a blank slate. The vault is the single
// canonical store (no second memory store — directive §13): the same
// records are readable by any vault-aware client on any host, and the
// agent's `search_notes` plane can find them once they carry a chat-turn
// kind tag. Content is XChaCha20-Poly1305 encrypted by vault-core before it
// ever touches the drive.

use serde::{Deserialize, Serialize};
use unoone_vault_core::{Record, RecordType, Vault};

use crate::documents;

/// Discriminator inside the decrypted MESSAGE content so recall never
/// mistakes another writer's MESSAGE records for chat turns.
pub const CHAT_TURN_KIND: &str = "chat_turn";
/// Bump when the ChatTurn content shape changes; recall accepts older
/// schemas where the fields still parse.
pub const CHAT_TURN_SCHEMA: u32 = 1;

/// One completed conversation turn, as stored (encrypted) in the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub kind: String,
    pub schema: u32,
    pub session_id: String,
    pub user_message: String,
    pub assistant_message: String,
    /// ISO 8601 (RFC 3339) — when the turn completed.
    pub timestamp: String,
}

impl ChatTurn {
    pub fn new(session_id: &str, user_message: &str, assistant_message: &str) -> Self {
        Self {
            kind: CHAT_TURN_KIND.to_string(),
            schema: CHAT_TURN_SCHEMA,
            session_id: session_id.to_string(),
            user_message: user_message.to_string(),
            assistant_message: assistant_message.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Write one turn to the unlocked vault as an encrypted MESSAGE record.
/// Returns the new record id.
pub fn save_chat_turn_to_vault(vault: &mut Vault, turn: &ChatTurn) -> Result<String, String> {
    let device_id = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "desktop-unknown".to_string());
    let record = Record::new(RecordType::Message, "DESKTOP", &device_id);
    let record_id = record.record_id.clone();
    let content =
        serde_json::to_vec(turn).map_err(|e| format!("Chat turn encode failed: {}", e))?;
    vault
        .write_record(record, &content)
        .map_err(|e| format!("Write failed: {}", e))?;
    Ok(record_id)
}

/// The decrypted, ordered tail of the conversation memory: the most recent
/// `limit` chat turns, oldest first (ready to splice into the UI/model
/// history). Skips tombstones (deletions stick) and any MESSAGE record that
/// is not a parseable chat turn — other MESSAGE writers must not surface in
/// the chat. Unreadable content is skipped, never invented.
pub fn recall_chat_turns(
    vault_root: &std::path::Path,
    vault: &Vault,
    limit: usize,
) -> Vec<ChatTurn> {
    let limit = limit.clamp(1, 500);
    let mut entries = documents::scan_record_metadata(vault_root)
        .into_iter()
        .filter(|entry| {
            !entry.tombstone
                && entry.parent_record_id.is_none()
                && entry.record_type == RecordType::Message
        })
        .collect::<Vec<_>>();
    // ISO 8601 RFC 3339 timestamps sort chronologically as strings.
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let start = entries.len().saturating_sub(limit);
    let mut turns = Vec::new();
    for entry in &entries[start..] {
        let Ok((record, plaintext)) = vault.read_record(&entry.record_id) else {
            continue;
        };
        if record.tombstone {
            continue;
        }
        match serde_json::from_slice::<ChatTurn>(&plaintext) {
            Ok(turn) if turn.kind == CHAT_TURN_KIND => turns.push(turn),
            _ => continue,
        }
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_vault(dir_path: &std::path::Path) -> (Vault, std::path::PathBuf) {
        let vault_root = dir_path.join("UNOONE");
        let _ = Vault::create(&vault_root, b"synthetic-chat-memory-pw").unwrap();
        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"synthetic-chat-memory-pw").unwrap();
        (vault, vault_root)
    }

    #[test]
    fn save_and_recall_round_trip_keeps_oldest_first_order() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = test_vault(dir.path());
        for index in 0..4 {
            let turn = ChatTurn::new(
                &format!("session-{index}"),
                &format!("question {index}"),
                &format!("answer {index}"),
            );
            save_chat_turn_to_vault(&mut vault, &turn).unwrap();
            // Distinct created_at timestamps keep the order deterministic
            // when the clock resolution collapses writes into one instant.
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let recalled = recall_chat_turns(&root, &vault, 10);
        assert_eq!(recalled.len(), 4);
        for (index, turn) in recalled.iter().enumerate() {
            assert_eq!(turn.user_message, format!("question {index}"));
            assert_eq!(turn.assistant_message, format!("answer {index}"));
            assert_eq!(turn.kind, CHAT_TURN_KIND);
        }
        // The limit keeps only the tail, oldest first.
        let tail = recall_chat_turns(&root, &vault, 2);
        assert_eq!(
            tail.iter()
                .map(|turn| turn.user_message.as_str())
                .collect::<Vec<_>>(),
            vec!["question 2", "question 3"]
        );
    }

    #[test]
    fn recall_skips_tombstones_and_foreign_message_records() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = test_vault(dir.path());
        let keep = ChatTurn::new("s1", "keep me", "kept");
        let keep_id = save_chat_turn_to_vault(&mut vault, &keep).unwrap();
        // A MESSAGE record from some other writer — must not surface as chat.
        let foreign = Record::new(RecordType::Message, "DESKTOP", "other");
        let foreign_id = foreign.record_id.clone();
        vault
            .write_record(foreign, br#"{"kind":"not_a_chat_turn"}"#)
            .unwrap();
        // A chat turn the user deleted — deletion sticks on every host.
        let deleted = ChatTurn::new("s1", "delete me", "deleted");
        let deleted_id = save_chat_turn_to_vault(&mut vault, &deleted).unwrap();
        vault
            .delete_record(&deleted_id, "DESKTOP", "chat-memory-test")
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let extra = ChatTurn::new("s2", "after delete", "still here");
        let extra_id = save_chat_turn_to_vault(&mut vault, &extra).unwrap();

        let recalled = recall_chat_turns(&root, &vault, 10);
        assert_eq!(recalled.len(), 2);
        assert_eq!(recalled[0].user_message, "keep me");
        assert_eq!(recalled[1].user_message, "after delete");
        // The ids came back from real vault writes.
        assert!(!keep_id.is_empty() && !extra_id.is_empty() && !foreign_id.is_empty());
    }
}
