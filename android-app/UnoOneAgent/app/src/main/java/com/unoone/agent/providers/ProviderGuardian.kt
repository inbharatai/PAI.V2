package com.unoone.agent.providers

import com.unoone.agent.core.guardian.*

/** Android mirror of Rust `personal-provider-adapters/src/guardian.rs`: declared Google connector manifest,
 * per-adapter egress policy (default offline), local-only guardian context and the mutation gate. */
internal object ProviderGuardian {
    const val GOOGLE_CONNECTOR_ID = "google-mail-calendar"
    val GOOGLE_ENDPOINTS = listOf("gmail.googleapis.com", "www.googleapis.com", "oauth2.googleapis.com", "accounts.google.com")
    fun googleManifest(account: String?, writeScopes: Boolean, now: Long) = ConnectorManifest(
        schema = CONNECTOR_MANIFEST_SCHEMA, manifest_version = CONNECTOR_MANIFEST_VERSION, connector_id = GOOGLE_CONNECTOR_ID,
        provider = "Google (Gmail and Calendar APIs)",
        purpose = if (writeScopes) "read, search, label, draft, send mail and create/update/cancel calendar events you review" else "read and search permitted mail and calendars",
        endpoints = GOOGLE_ENDPOINTS,
        permitted_operations = if (writeScopes) listOf("read", "search", "label", "save_draft", "send", "create_event", "update_event", "cancel_event") else listOf("read", "search"),
        accounts = listOfNotNull(account),
        data_fields = listOf("search query", "label ids", "message/thread ids", "reviewed draft: recipients, subject, body", "reviewed event: title, start/end, time zone, attendees"),
        token_scopes = if (writeScopes) READ_SCOPES + listOf("https://www.googleapis.com/auth/gmail.compose", "https://www.googleapis.com/auth/gmail.modify", "https://www.googleapis.com/auth/calendar.events") else READ_SCOPES,
        retention = "Google retains mail/calendar data under the account's own Google terms; this app stores only short-lived tokens and reviewed receipts locally",
        expected_cost = "none charged by this app; provider quota only", max_request_bytes = 64 * 1024, max_requests_per_day = 2000,
        expires_at_ms = now + 30L * 86_400_000, revocation = "Disconnect revokes access at Google (best effort), erases local tokens and rejects prepared operations; provider-side deletion of mail is not promised",
    )

    /** Consent exists only because the person authorized the account through the official SDK. */
    class Egress(account: String, writeScopes: Boolean, now: Long) {
        private val manifest = googleManifest(account, writeScopes, now)
        private val policy = EgressPolicy().also { it.consent(ConnectorConsent.grant(manifest, now)) }
        fun authorize(url: String, bytes: Long, now: Long) { synchronized(policy) { policy.authorize(listOf(manifest), url, bytes, now) } }
    }

    /** Known contacts = recipients with a prior VERIFIED provider readback; trusted domain = own account domain. */
    fun context(state: ProviderLocalState): Context {
        val known = state.entries.filter { it.status == CommitStatus.VERIFIED }.flatMap {
            when (val m = it.review.mutation) { is ProviderMutation.Send -> m.draft.to; is ProviderMutation.CreateEvent -> m.event.attendees; is ProviderMutation.UpdateEvent -> m.event.attendees; else -> emptyList() }
        }.distinct().sorted()
        val trusted = listOfNotNull(state.tokens?.account?.substringAfterLast('@', "")?.ifEmpty { null })
        return Context(knownContacts = known, trustedDomains = trusted)
    }
    fun intent(review: Review): Intent? = when (val m = review.mutation) {
        is ProviderMutation.Send -> Intent.SendMessage(m.draft.to, m.draft.subject, m.draft.body, m.draft.reply != null, emptyList())
        is ProviderMutation.SaveDraft -> Intent.SendMessage(m.draft.to, m.draft.subject, m.draft.body, m.draft.reply != null, emptyList())
        is ProviderMutation.CreateEvent -> Intent.SendMessage(m.event.attendees, m.event.summary, "${m.event.start} – ${m.event.end} ${m.event.time_zone}", false, emptyList())
        is ProviderMutation.UpdateEvent -> Intent.SendMessage(m.event.attendees, m.event.summary, "${m.event.start} – ${m.event.end} ${m.event.time_zone}", true, emptyList())
        is ProviderMutation.Label, is ProviderMutation.CancelEvent -> null
    }
    /** Saving a draft has no recipient effect: WARN downgrades to ALLOW, BLOCK stays (secrets never go to the provider). */
    fun decision(review: Review, ctx: Context): Decision? {
        val d = PrivacyGuardian.check(intent(review) ?: return null, ctx)
        return if (review.mutation is ProviderMutation.SaveDraft && d.severity == Severity.WARN) d.copy(severity = Severity.ALLOW, explanation = "Draft only, nothing sent. ${d.explanation}") else d
    }
    /** Throws GuardianRefusal on BLOCK or unacknowledged/mismatched WARN. */
    fun enforce(review: Review, ctx: Context, acknowledgedFingerprint: String?, now: Long): Receipt? {
        val d = decision(review, ctx) ?: return null
        val ack = acknowledgedFingerprint?.takeIf { it == d.fingerprint }?.let { Acknowledgement.byHuman(d, now) }
        return PrivacyGuardian.enforce(d, ack, now)
    }
}
