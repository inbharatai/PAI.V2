package com.unoone.agent.providers

import kotlinx.serialization.*
import kotlinx.serialization.json.*
import java.security.MessageDigest
import java.time.OffsetDateTime
import java.time.ZoneId
import java.util.UUID
import java.util.Base64

internal val providerJson = Json { encodeDefaults = true; ignoreUnknownKeys = false; classDiscriminator = "operation" }
internal const val REVIEW_TTL = 300_000L
internal val READ_SCOPES = listOf("https://www.googleapis.com/auth/gmail.readonly", "https://www.googleapis.com/auth/calendar.readonly")
@Serializable data class MailAccount(val email: String, val provider: String = "GOOGLE")
@Serializable data class MessageRef(val id: String, val threadId: String, val labelIds: List<String> = emptyList(), val snippet: String = "")
@Serializable data class CalendarRef(val id: String, val summary: String = "", val timeZone: String = "")
@Serializable data class EventRef(val id: String, val etag: String, val status: String, val summary: String = "")
@Serializable data class Reply(val thread_id: String, val message_id: String, val references: String)
@Serializable data class Draft(val to: List<String>, val subject: String, val body: String, val reply: Reply? = null) {
    fun validate() { require(to.size in 1..10 && body.toByteArray().size <= 16384); to.forEach(::address); header(subject,256); reply?.let { identifier(it.thread_id); header(it.message_id,512); header(it.references,2048) } }
    fun raw(account: String, operation: String): String {
        validate(); address(account); UUID.fromString(operation)
        val b64 = Base64.getEncoder()
        val headers = "From: $account\r\nTo: ${to.joinToString(", ")}\r\nSubject: =?UTF-8?B?${b64.encodeToString(subject.toByteArray())}?=\r\nMessage-ID: <$operation@unoone.local>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n" +
            (reply?.let { "In-Reply-To: ${it.message_id}\r\nReferences: ${it.references}\r\n" } ?: "") + "\r\n" + b64.encodeToString(body.toByteArray())
        return Base64.getUrlEncoder().withoutPadding().encodeToString(headers.toByteArray())
    }
}
@Serializable data class EventDraft(val summary: String, val start: String, val end: String, val time_zone: String, val attendees: List<String>) {
    fun validate() { header(summary,256); interval(start,end); ZoneId.of(time_zone); require(attendees.size<=10); attendees.forEach(::address) }
    fun body(): JsonObject = buildJsonObject { put("summary",summary); put("start",buildJsonObject { put("dateTime",start); put("timeZone",time_zone) }); put("end",buildJsonObject { put("dateTime",end); put("timeZone",time_zone) }); put("attendees",JsonArray(attendees.map { buildJsonObject { put("email",it) } })) }
}
@Serializable sealed class ProviderMutation {
    @Serializable @SerialName("SAVE_DRAFT") data class SaveDraft(val draft: Draft): ProviderMutation()
    @Serializable @SerialName("SEND") data class Send(val draft: Draft): ProviderMutation()
    @Serializable @SerialName("LABEL") data class Label(val message_id: String, val add: List<String>, val remove: List<String>): ProviderMutation()
    @Serializable @SerialName("CREATE_EVENT") data class CreateEvent(val event: EventDraft): ProviderMutation()
    @Serializable @SerialName("UPDATE_EVENT") data class UpdateEvent(val event_id: String, val etag: String, val event: EventDraft): ProviderMutation()
    @Serializable @SerialName("CANCEL_EVENT") data class CancelEvent(val event_id: String, val etag: String, val event: EventDraft): ProviderMutation()
}
@Serializable data class Review(val operation_id: String, val task_id: String, val account: String, val container: String, val owner_replica: String, val prepared_ms: Long, val mutation: ProviderMutation) {
    fun digest(): String = MessageDigest.getInstance("SHA-256").digest(providerJson.encodeToString(this).toByteArray()).joinToString("") { "%02x".format(it) }
    fun eventId() = "u" + operation_id.replace("-", "")
    fun validate(now: Long) {
        listOf(operation_id,task_id,owner_replica).forEach { require(UUID.fromString(it).toString()==it) }; address(account); identifier(container)
        require(now>=prepared_ms && now-prepared_ms<REVIEW_TTL) { "Stale review; prepare again" }
        when(val m=mutation) {
            is ProviderMutation.Send -> m.draft.validate(); is ProviderMutation.SaveDraft -> m.draft.validate()
            is ProviderMutation.CreateEvent -> m.event.validate()
            is ProviderMutation.UpdateEvent -> { identifier(m.event_id); header(m.etag,256); m.event.validate() }
            is ProviderMutation.CancelEvent -> { identifier(m.event_id); header(m.etag,256); m.event.validate() }
            is ProviderMutation.Label -> { identifier(m.message_id); require((m.add+m.remove).size in 1..20 && m.add.none { it in m.remove }); (m.add+m.remove).forEach { identifier(it); require(it !in listOf("TRASH","SPAM","SENT","DRAFT")) } }
        }
    }
}
/** Private construction, no serialization, no provider/model/peer approval entrypoint. */
internal class CapabilityGrant private constructor(private val digest: String, private val account: String, private val expires: Long) {
    fun check(review: Review, actualAccount: String, now: Long) { review.validate(now); require(now<expires && account==actualAccount && review.account==account && review.digest()==digest) { "Grant expired or exact scope changed" } }
    companion object { fun fromNativeReview(review: Review, exactDigest: String, now: Long): CapabilityGrant { review.validate(now); require(review.digest()==exactDigest); return CapabilityGrant(exactDigest,review.account,now+REVIEW_TTL) } }
}
@Serializable enum class CommitStatus { PREPARED, NEEDS_RECONCILIATION, VERIFIED, REJECTED }
@Serializable data class ProviderReceipt(val operation_id: String, val task_id: String, val account: String, val provider_id: String, val container: String, val status: CommitStatus, val observed_ms: Long, val detail: String)
@Serializable data class PreparedEntry(val review: Review, val digest: String, var status: CommitStatus = CommitStatus.PREPARED, var receipt: ProviderReceipt? = null)
internal fun header(value: String,max: Int) { require(value.isNotEmpty() && value.toByteArray().size<=max && value.none(Char::isISOControl)) }
internal fun identifier(value: String) { header(value,512); require(value !in listOf("*",".","..")) }
internal fun address(value: String) { header(value,254); require(value.matches(Regex("[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+\\.[A-Za-z0-9-]+"))) }
internal fun interval(start: String,end: String) { val a=OffsetDateTime.parse(start);val b=OffsetDateTime.parse(end);require(b.isAfter(a) && java.time.Duration.between(a,b).toDays()<=31) }
internal fun JsonObject.string(key: String) = get(key)?.jsonPrimitive?.contentOrNull ?: error("Provider field missing")
internal fun JsonObject.array(key: String): JsonArray = get(key)?.jsonArray ?: JsonArray(emptyList())
