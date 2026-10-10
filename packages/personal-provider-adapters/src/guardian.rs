//! Narrow privacy-guardian hooks at the provider boundary (brief §3.6).
//! * Declared connector manifest for the Google route; every outbound URL must match it.
//! * Mutation dispatch (send/invite/draft) runs the host-owned guardian check; WARN needs a fresh
//!   human acknowledgement of the exact fingerprint, BLOCK never dispatches.
//!
//! Provider text remains untrusted data and supplies no context here.
use crate::{store::LocalState, types::*, Result};
use unoone_personal_agent_runtime as runtime;
use unoone_privacy_guardian as g;

pub const GOOGLE_CONNECTOR_ID: &str = "google-mail-calendar";
/// Exact hosts this adapter may ever contact. `oauth.rs` and `google.rs` build URLs only on these.
pub const GOOGLE_ENDPOINTS: [&str; 4] = [
    "gmail.googleapis.com",
    "www.googleapis.com",
    "oauth2.googleapis.com",
    "accounts.google.com",
];

/// Reviewed manifest shown in the consent preview before Connect. `account` is empty until connected.
pub fn google_manifest(
    account: Option<&str>,
    write_scopes: bool,
    now: u64,
) -> g::connector::ConnectorManifest {
    g::connector::ConnectorManifest {
        schema: g::connector::MANIFEST_SCHEMA.into(),
        manifest_version: g::connector::MANIFEST_VERSION,
        connector_id: GOOGLE_CONNECTOR_ID.into(),
        provider: "Google (Gmail and Calendar APIs)".into(),
        purpose: if write_scopes { "read, search, label, draft, send mail and create/update/cancel calendar events you review" } else { "read and search permitted mail and calendars" }.into(),
        endpoints: GOOGLE_ENDPOINTS.iter().map(|s| s.to_string()).collect(),
        permitted_operations: if write_scopes { vec!["read".into(), "search".into(), "label".into(), "save_draft".into(), "send".into(), "create_event".into(), "update_event".into(), "cancel_event".into()] } else { vec!["read".into(), "search".into()] },
        accounts: account.map(|a| vec![a.to_string()]).unwrap_or_default(),
        data_fields: vec!["search query".into(), "label ids".into(), "message/thread ids".into(), "reviewed draft: recipients, subject, body".into(), "reviewed event: title, start/end, time zone, attendees".into()],
        token_scopes: if write_scopes { vec![SCOPES_READ[0].into(), SCOPES_READ[1].into(), "https://www.googleapis.com/auth/gmail.compose".into(), "https://www.googleapis.com/auth/gmail.modify".into(), "https://www.googleapis.com/auth/calendar.events".into()] } else { SCOPES_READ.iter().map(|s| s.to_string()).collect() },
        retention: "Google retains mail/calendar data under the account's own Google terms; this app stores only tokens and reviewed receipts locally".into(),
        expected_cost: "none charged by this app; provider quota only".into(),
        max_request_bytes: 64 * 1024,
        max_requests_per_day: 2000,
        expires_at_ms: now + 30 * 86_400_000,
        revocation: "Disconnect revokes the token at Google (best effort), erases local tokens and rejects prepared operations; provider-side deletion of mail is not promised".into(),
    }
}

/// Outbound policy for one adapter instance: consent exists only because the person connected an
/// account; nothing is enabled before that. Loopback http is accepted ONLY for stub-provider tests.
pub struct Egress {
    manifest: g::connector::ConnectorManifest,
    policy: std::sync::Mutex<g::connector::EgressPolicy>,
}
impl Egress {
    pub fn for_connected_account(account: &str, write_scopes: bool, now: u64) -> Result<Self> {
        let manifest = google_manifest(Some(account), write_scopes, now);
        let mut policy = g::connector::EgressPolicy::offline();
        policy.consent(g::connector::ConnectorConsent::grant(&manifest, now)?);
        Ok(Self {
            manifest,
            policy: std::sync::Mutex::new(policy),
        })
    }
    pub fn authorize(&self, url: &reqwest::Url, bytes: u64, now: u64) -> Result<()> {
        let loopback = url.scheme() == "http"
            && matches!(
                url.host_str(),
                Some("127.0.0.1") | Some("localhost") | Some("[::1]")
            );
        if loopback {
            // Stub provider in tests: still enforce byte ceiling; never a cloud fallback.
            return if bytes <= self.manifest.max_request_bytes {
                Ok(())
            } else {
                Err("Egress refused: byte ceiling".into())
            };
        }
        self.policy
            .lock()
            .map_err(|_| "Egress policy lock")?
            .authorize(
                std::slice::from_ref(&self.manifest),
                url.as_str(),
                bytes,
                now,
            )
            .map(|_| ())
    }
}

/// Context assembled from LOCAL evidence only: recipients the person already sent to with a
/// VERIFIED provider readback, and the account's own domain. Provider/message text is never used.
pub fn context(local: &LocalState) -> g::Context {
    let mut known = Vec::new();
    for e in &local.entries {
        if e.status != CommitStatus::Verified {
            continue;
        }
        match &e.review.mutation {
            Mutation::Send { draft } => known.extend(draft.to.iter().cloned()),
            Mutation::CreateEvent { event } | Mutation::UpdateEvent { event, .. } => {
                known.extend(event.attendees.iter().cloned())
            }
            _ => {}
        }
    }
    known.sort();
    known.dedup();
    let trusted = local
        .tokens
        .as_ref()
        .and_then(|t| t.account().rsplit_once('@').map(|(_, d)| d.to_string()))
        .into_iter()
        .collect();
    g::Context {
        known_contacts: known,
        trusted_domains: trusted,
        ..Default::default()
    }
}

/// Typed intent for a reviewed mutation. Label/cancel have no new destination and return None.
pub fn intent(review: &Review) -> Option<g::Intent> {
    match &review.mutation {
        Mutation::Send { draft } | Mutation::SaveDraft { draft } => Some(g::Intent::SendMessage {
            recipients: draft.to.clone(),
            subject: draft.subject.clone(),
            body: draft.body.clone(),
            reply_in_known_thread: draft.reply.is_some(),
            attachments: vec![],
        }),
        Mutation::CreateEvent { event } | Mutation::UpdateEvent { event, .. } => {
            Some(g::Intent::SendMessage {
                recipients: event.attendees.clone(),
                subject: event.summary.clone(),
                body: format!("{} – {} {}", event.start, event.end, event.time_zone),
                reply_in_known_thread: matches!(review.mutation, Mutation::UpdateEvent { .. }),
                attachments: vec![],
            })
        }
        Mutation::Label { .. } | Mutation::CancelEvent { .. } => None,
    }
}

/// Decision for the UI to show BEFORE commit. Saving a draft (no recipient effect) downgrades WARN to
/// ALLOW but keeps BLOCK (secrets never go to the provider either).
pub fn decision(review: &Review, ctx: &g::Context) -> Option<g::Decision> {
    let mut d = g::check(&intent(review)?, ctx);
    if matches!(review.mutation, Mutation::SaveDraft { .. }) && d.severity == g::Severity::Warn {
        d.severity = g::Severity::Allow;
        d.explanation = format!("Draft only, nothing sent. {}", d.explanation);
    }
    Some(d)
}

/// Enforcement used by commit. `acknowledged_fingerprint` is the exact fingerprint the UI showed and the
/// human accepted; anything else is not an acknowledgement.
pub fn enforce(
    review: &Review,
    ctx: &g::Context,
    acknowledged_fingerprint: Option<&str>,
    now: u64,
) -> Result<Option<g::Receipt>> {
    let Some(d) = decision(review, ctx) else {
        return Ok(None);
    };
    let ack = acknowledged_fingerprint
        .filter(|f| *f == d.fingerprint)
        .map(|_| g::Acknowledgement::by_human(&d, now));
    g::enforce(&d, ack.as_ref(), now)
        .map(Some)
        .map_err(|r| r.message)
}

/// Writes the guardian receipt/correction next to the task via the runtime's tiny API.
pub fn task_note(
    vault: &mut unoone_vault_core::Vault,
    task_id: &str,
    note: &str,
    now: u64,
) -> Result<()> {
    let mut ledger = runtime::load(vault)?;
    let revision = ledger.view()?.revision;
    ledger.record_guardian_note(revision, task_id, note, now)?;
    runtime::save(vault, &ledger)
}
