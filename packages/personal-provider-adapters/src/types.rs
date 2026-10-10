use crate::{ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_BODY: usize = 16 * 1024;
pub const MAX_RESPONSE: usize = 1024 * 1024;
pub const MAX_PAGE: usize = 50;
pub const REVIEW_LIFETIME_MS: u64 = 5 * 60 * 1000;
pub const SCOPES_READ: [&str; 2] = [
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/calendar.readonly",
];

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MailAccount {
    pub email: String,
    pub provider: String,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: String,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct CalendarRef {
    pub id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(rename = "timeZone", default)]
    pub time_zone: String,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct EventRef {
    pub id: String,
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub summary: String,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub thread_id: String,
    pub message_id: String,
    pub references: String,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    pub reply: Option<Reply>,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventDraft {
    pub summary: String,
    pub start: String,
    pub end: String,
    pub time_zone: String,
    pub attendees: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(
    tag = "operation",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum Mutation {
    SaveDraft {
        draft: Draft,
    },
    Send {
        draft: Draft,
    },
    Label {
        message_id: String,
        add: Vec<String>,
        remove: Vec<String>,
    },
    CreateEvent {
        event: EventDraft,
    },
    UpdateEvent {
        event_id: String,
        etag: String,
        event: EventDraft,
    },
    CancelEvent {
        event_id: String,
        etag: String,
        event: EventDraft,
    },
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub operation_id: String,
    pub task_id: String,
    pub account: String,
    /// Exact Gmail label ID or Calendar ID, never an account-wide wildcard.
    pub container: String,
    pub owner_replica: String,
    pub prepared_ms: u64,
    pub mutation: Mutation,
}
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommitStatus {
    Prepared,
    NeedsReconciliation,
    Verified,
    Rejected,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ProviderReceipt {
    pub operation_id: String,
    pub task_id: String,
    pub account: String,
    pub provider_id: String,
    pub container: String,
    pub status: CommitStatus,
    pub observed_ms: u64,
    pub detail: String,
}

/// Constructor is called only by native human-review IPC. Not deserializable from a
/// model or peer. Digest includes every field, account, folder/calendar and recipients.
pub struct CapabilityGrant {
    digest: String,
    account: String,
    expires: u64,
}
impl CapabilityGrant {
    pub fn from_native_review(review: &Review, exact_digest: &str, now: u64) -> Result<Self> {
        review.validate(now)?;
        let digest = review.digest()?;
        ensure(
            digest == exact_digest,
            "Review changed; review exact content again",
        )?;
        Ok(Self {
            digest,
            account: review.account.clone(),
            expires: now + REVIEW_LIFETIME_MS,
        })
    }
    pub fn check(&self, review: &Review, account: &str, now: u64) -> Result<()> {
        review.validate(now)?;
        ensure(
            self.account == account
                && review.account == account
                && self.digest == review.digest()?
                && now < self.expires,
            "Grant expired, revoked or outside exact review scope",
        )
    }
}
fn header(s: &str, max: usize) -> Result<()> {
    ensure(
        !s.is_empty() && s.len() <= max && !s.chars().any(|c| c.is_control()),
        "Invalid/oversized header",
    )
}
pub fn address(s: &str) -> Result<()> {
    header(s, 254)?;
    let parts: Vec<_> = s.split('@').collect();
    ensure(
        parts.len() == 2
            && !parts[0].is_empty()
            && parts[1].contains('.')
            && s.is_ascii()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~@".contains(&b)),
        "Use an exact email address, without display names",
    )
}
pub fn identifier(s: &str) -> Result<()> {
    ensure(
        !s.is_empty()
            && s.len() <= 512
            && !s.chars().any(|c| c.is_control())
            && s != "*"
            && s != "."
            && s != "..",
        "Invalid provider identifier",
    )
}
impl Draft {
    pub fn validate(&self) -> Result<()> {
        ensure(
            !self.to.is_empty() && self.to.len() <= 10 && self.body.len() <= MAX_BODY,
            "Mail size/recipient limit; attachments are not supported",
        )?;
        for recipient in &self.to {
            address(recipient)?;
        }
        header(&self.subject, 256)?;
        if let Some(reply) = &self.reply {
            identifier(&reply.thread_id)?;
            header(&reply.message_id, 512)?;
            header(&reply.references, 2048)?;
        }
        Ok(())
    }
    pub fn raw(&self, account: &str, operation: &str) -> Result<String> {
        self.validate()?;
        address(account)?;
        uuid::Uuid::parse_str(operation).map_err(|_| "Invalid operation ID")?;
        let subject = base64::engine::general_purpose::STANDARD.encode(self.subject.as_bytes());
        let mut raw = format!("From: {account}\r\nTo: {}\r\nSubject: =?UTF-8?B?{subject}?=\r\nMessage-ID: <{operation}@unoone.local>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n", self.to.join(", "));
        if let Some(reply) = &self.reply {
            raw.push_str(&format!(
                "In-Reply-To: {}\r\nReferences: {}\r\n",
                reply.message_id, reply.references
            ));
        }
        raw.push_str("\r\n");
        raw.push_str(&base64::engine::general_purpose::STANDARD.encode(self.body.as_bytes()));
        Ok(URL_SAFE_NO_PAD.encode(raw))
    }
}
impl EventDraft {
    pub fn validate(&self) -> Result<()> {
        header(&self.summary, 256)?;
        header(&self.time_zone, 128)?;
        let start = DateTime::parse_from_rfc3339(&self.start)
            .map_err(|_| "Start requires RFC3339 with explicit offset")?;
        let end = DateTime::parse_from_rfc3339(&self.end)
            .map_err(|_| "End requires RFC3339 with explicit offset")?;
        ensure(
            end > start && (end - start).num_days() <= 31,
            "Event interval must be positive and at most 31 days",
        )?;
        ensure(self.attendees.len() <= 10, "At most 10 reviewed attendees")?;
        for recipient in &self.attendees {
            address(recipient)?;
        }
        Ok(())
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"summary":self.summary,"start":{"dateTime":self.start,"timeZone":self.time_zone},"end":{"dateTime":self.end,"timeZone":self.time_zone},"attendees":self.attendees.iter().map(|email|serde_json::json!({"email":email})).collect::<Vec<_>>()})
    }
}
impl Review {
    pub fn digest(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|_| "Review encoding failed")?)
        ))
    }
    pub fn event_id(&self) -> Result<String> {
        Ok(format!("u{}", self.operation_id.replace('-', "")))
    }
    pub fn validate(&self, now: u64) -> Result<()> {
        for id in [&self.operation_id, &self.task_id, &self.owner_replica] {
            uuid::Uuid::parse_str(id).map_err(|_| "Invalid local identifier")?;
        }
        address(&self.account)?;
        identifier(&self.container)?;
        ensure(
            now >= self.prepared_ms && now - self.prepared_ms < REVIEW_LIFETIME_MS,
            "Stale review; prepare again",
        )?;
        match &self.mutation {
            Mutation::Send { draft } | Mutation::SaveDraft { draft } => draft.validate(),
            Mutation::CreateEvent { event } => event.validate(),
            Mutation::UpdateEvent {
                event_id,
                etag,
                event,
            } => {
                identifier(event_id)?;
                header(etag, 256)?;
                event.validate()
            }
            Mutation::CancelEvent {
                event_id,
                etag,
                event,
            } => {
                identifier(event_id)?;
                header(etag, 256)?;
                event.validate()
            }
            Mutation::Label {
                message_id,
                add,
                remove,
            } => {
                identifier(message_id)?;
                ensure(
                    add.len() + remove.len() <= 20 && !(add.is_empty() && remove.is_empty()),
                    "Label limit",
                )?;
                for label in add.iter().chain(remove) {
                    identifier(label)?;
                    ensure(
                        !["TRASH", "SPAM", "SENT", "DRAFT"].contains(&label.as_str()),
                        "Deletion/spam/send labels not supported",
                    )?;
                }
                ensure(
                    !add.iter().any(|a| remove.contains(a)),
                    "Contradictory labels",
                )
            }
        }
    }
}
