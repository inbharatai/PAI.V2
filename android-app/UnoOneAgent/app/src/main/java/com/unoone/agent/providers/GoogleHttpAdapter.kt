package com.unoone.agent.providers

import kotlinx.serialization.json.*
import java.net.URL
import java.net.URLEncoder
import javax.net.ssl.HttpsURLConnection
import java.util.Base64

/** Actual Android HTTPS transport. No arbitrary URL, redirects, attachment fetching,
 * logging, automatic pagination/retries or interpretation of email text as authority. */
internal class GoogleHttpAdapter(
    private val token: String, val account: String, scopes: Set<String>,
    private val now: () -> Long = System::currentTimeMillis,
    private val sessionEpoch: () -> Long = com.unoone.agent.vaultbridge.VaultConnection::sessionEpoch,
    private val openConnection: (String) -> HttpsURLConnection = { URL(it).openConnection() as HttpsURLConnection }
) {
    private val scopes = scopes.toSet()
    private val epoch = sessionEpoch()
    /** §3.6 declared-connector egress policy; every URL this adapter contacts passes through it. */
    private val egress = ProviderGuardian.Egress(account, scopes.any { it !in READ_SCOPES }, now())
    /** Guardian context + the exact acknowledged WARN fingerprint (null = none). Set by native review only. */
    private var guardianContext: com.unoone.agent.core.guardian.Context = com.unoone.agent.core.guardian.Context()
    private var acknowledgedFingerprint: String? = null
    fun withGuardian(ctx: com.unoone.agent.core.guardian.Context, acknowledged: String?): GoogleHttpAdapter { guardianContext = ctx; acknowledgedFingerprint = acknowledged; return this }
    // Defense in depth only. ProviderService persists NEEDS_RECONCILIATION before invoking
    // commit; this process-local set never replaces that durable no-replay reservation.
    private val attemptedOperations = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    private fun scope(value: String) = require(value in scopes) { "Authorize required provider permission first" }
    private fun url(calendar: Boolean, path: List<String>, query: Map<String,String> = emptyMap()): String {
        val encode: (String)->String = { URLEncoder.encode(it,"UTF-8").replace("+","%20") }
        path.forEach(::identifier)
        return (if(calendar) "https://www.googleapis.com/calendar/v3/" else "https://gmail.googleapis.com/gmail/v1/users/me/") + path.joinToString("/"){encode(it)} + if(query.isEmpty()) "" else query.entries.joinToString("&","?"){encode(it.key)+"="+encode(it.value)}
    }
    private fun call(calendar: Boolean, path: List<String>, method: String = "GET", body: JsonObject? = null, query: Map<String,String> = emptyMap(), etag: String? = null, mutationAuthorization: (() -> Unit)? = null): JsonObject {
        check(sessionEpoch()==epoch){"Vault session revoked; no further request"}
        // freeBusy is a read-only POST. Every effectful request must carry its exact operation guard.
        val mutation = method != "GET" && !(calendar && path == listOf("freeBusy") && method == "POST")
        check(!mutation || (mutationAuthorization != null && body != null)) { "Mutation requires an exact dispatch authorization and body" }
        val target=url(calendar,path,query)
        // Outbound network policy: destination must match the declared connector manifest (default offline otherwise).
        egress.authorize(target,(body?.toString()?.toByteArray()?.size?:0).toLong(),now())
        val connection=openConnection(target)
        connection.instanceFollowRedirects=false; connection.connectTimeout=10_000; connection.readTimeout=25_000; connection.requestMethod=method
        connection.setRequestProperty("Authorization","Bearer $token");connection.setRequestProperty("Accept","application/json")
        if(etag!=null) connection.setRequestProperty("If-Match",etag)
        val deadline=System.nanoTime()+25_000_000_000L
        val watchdog=java.util.Timer(true)
        watchdog.schedule(object:java.util.TimerTask(){override fun run(){connection.disconnect()}},25_000L)
        try {
            if(body!=null) { val bytes=body.toString().toByteArray(); require(bytes.size<=65536); connection.doOutput=true; connection.setRequestProperty("Content-Type","application/json"); connection.setFixedLengthStreamingMode(bytes.size)
                // Last check before the first possible network effect, including after preflight GETs
                // and transport setup. A failed/uncertain attempt is not automatically retried.
                check(sessionEpoch()==epoch) { "Vault session revoked; no mutation dispatched" }
                if(mutation) checkNotNull(mutationAuthorization).invoke()
                connection.outputStream.use { it.write(bytes) } }
            val code=connection.responseCode
            check(code in 200..299) { when(code){401->"Authorization expired/revoked";403->"Provider permission denied";409,412->"Provider changed; review again";429->"Rate limited; no automatic retry";else->"Provider request failed; mutations require reconciliation"} }
            require(connection.contentLengthLong<=1048576) { "Response size bound" }
            val out=java.io.ByteArrayOutputStream(); connection.inputStream.use { input -> val bytes=ByteArray(8192); while(true) { check(System.nanoTime()<deadline){"Response timeout; reconcile mutation"};val n=input.read(bytes);if(n<0)break;require(out.size()+n<=1048576);out.write(bytes,0,n) } }
            val bytes=out.toByteArray();return try { providerJson.parseToJsonElement(bytes.toString(Charsets.UTF_8)).jsonObject } finally { bytes.fill(0) }
        } catch(e: IllegalArgumentException) { throw IllegalStateException("Provider response/argument invalid; uncertain mutation must reconcile") }
        catch(e: java.io.IOException) { throw IllegalStateException("Provider connection interrupted; uncertain mutation must reconcile") }
        finally { watchdog.cancel(); connection.disconnect() }
    }
    fun profile(): String = call(false,listOf("profile")).string("emailAddress").also(::address)
    fun search(folder: String,query: String,page: String? = null): JsonObject { scope(READ_SCOPES[0]);identifier(folder);require(query.toByteArray().size<=1024);val q=mutableMapOf("labelIds" to folder,"q" to query,"maxResults" to "50","includeSpamTrash" to "false");page?.let{require(it.length<=2048);q["pageToken"]=it};return call(false,listOf("messages"),query=q).also { require(it.array("messages").size<=50) } }
    fun labels(): JsonObject { scope(READ_SCOPES[0]); return call(false,listOf("labels")) }
    fun message(folder: String,id: String): JsonObject { scope(READ_SCOPES[0]);return call(false,listOf("messages",id),query=mapOf("format" to "full")).also { require(it.string("id")==id && it.array("labelIds").any { label->label.jsonPrimitive.content==folder }){"Message outside scoped folder"} } }
    fun thread(folder: String,id: String): JsonObject { scope(READ_SCOPES[0]);return call(false,listOf("threads",id),query=mapOf("format" to "full")).also { require(it.array("messages").size<=20 && it.array("messages").all { m->m.jsonObject.array("labelIds").any { label->label.jsonPrimitive.content==folder } }){"Thread outside scope or reply limit"} } }
    fun calendars(page: String? = null): JsonObject {scope(READ_SCOPES[1]);val query=mutableMapOf("maxResults" to "50");page?.let{require(it.length<=2048);query["pageToken"]=it};return call(true,listOf("users","me","calendarList"),query=query).also { require(it.array("items").size<=50) }}
    fun events(calendar: String,start: String,end: String,page: String? = null): JsonObject {scope(READ_SCOPES[1]);interval(start,end);val q=mutableMapOf("maxResults" to "50","singleEvents" to "true","timeMin" to start,"timeMax" to end);page?.let {require(it.length<=2048);q["pageToken"]=it};return call(true,listOf("calendars",calendar,"events"),query=q).also { require(it.array("items").size<=50) }}
    fun event(calendar: String,id: String): JsonObject {scope(READ_SCOPES[1]);return call(true,listOf("calendars",calendar,"events",id))}
    fun freeBusy(calendar: String,start: String,end: String): JsonObject {scope(READ_SCOPES[1]);interval(start,end);identifier(calendar);val result=call(true,listOf("freeBusy"),"POST",buildJsonObject {put("timeMin",start);put("timeMax",end);put("calendarExpansionMax",1);put("groupExpansionMax",1);put("items",buildJsonArray { add(buildJsonObject { put("id",calendar) }) }) })
        val data=result["calendars"]?.jsonObject?.get(calendar)?.jsonObject ?: error("Availability unknown")
        require(data.array("errors").isEmpty() && data["busy"] is JsonArray) {"Availability unknown, not free"};return result }
    fun commit(review: Review,grant: CapabilityGrant): ProviderReceipt {
        grant.check(review,account,now())
        // Host-owned guardian gate: BLOCK never dispatches; WARN needs the exact acknowledged fingerprint.
        ProviderGuardian.enforce(review,guardianContext,acknowledgedFingerprint,now())
        check(attemptedOperations.add(review.operation_id)) { "Already attempted; reconcile, never replay" }
        val authorizeMutation = { grant.check(review,account,now()) }
        val value=when(val m=review.mutation) {
            is ProviderMutation.Send,is ProviderMutation.SaveDraft -> {
                scope("https://www.googleapis.com/auth/gmail.compose")
                val draft=if(m is ProviderMutation.Send)m.draft else (m as ProviderMutation.SaveDraft).draft
                draft.reply?.let {reply->val original=thread(review.container,reply.thread_id);require(original.array("messages").any {m->m.jsonObject["payload"]?.jsonObject?.array("headers")?.any {h->h.jsonObject["name"]?.jsonPrimitive?.content?.equals("Message-ID",ignoreCase=true)==true && h.jsonObject["value"]?.jsonPrimitive?.content==reply.message_id}==true}){"Reply Message-ID is not in permitted provider thread"}}
                val message=buildJsonObject {put("raw",draft.raw(account,review.operation_id));draft.reply?.let {put("threadId",it.thread_id)}}
                grant.check(review,account,now())
                if(m is ProviderMutation.Send)call(false,listOf("messages","send"),"POST",message,mutationAuthorization=authorizeMutation) else call(false,listOf("drafts"),"POST",buildJsonObject {put("message",message)},mutationAuthorization=authorizeMutation)
            }
            is ProviderMutation.Label -> {scope("https://www.googleapis.com/auth/gmail.modify");message(review.container,m.message_id);call(false,listOf("messages",m.message_id,"modify"),"POST",buildJsonObject {put("addLabelIds",JsonArray(m.add.map(::JsonPrimitive)));put("removeLabelIds",JsonArray(m.remove.map(::JsonPrimitive)))},mutationAuthorization=authorizeMutation)}
            is ProviderMutation.CreateEvent,is ProviderMutation.UpdateEvent -> {
                scope("https://www.googleapis.com/auth/calendar.events")
                val event=if(m is ProviderMutation.CreateEvent)m.event else (m as ProviderMutation.UpdateEvent).event
                val busy=freeBusy(review.container,event.start,event.end)
                if(m is ProviderMutation.UpdateEvent) {
                    val old=event(review.container,m.event_id);require(old.string("etag")==m.etag && old.array("attendees").all {it.jsonObject.string("email") in event.attendees}){"Changed event or unsupported attendee removal"}
                    val overlaps=events(review.container,event.start,event.end);require("nextPageToken" !in overlaps && overlaps.array("items").all {it.jsonObject.string("id")==m.event_id || it.jsonObject["status"]==JsonPrimitive("cancelled") || it.jsonObject["transparency"]==JsonPrimitive("transparent")}){"Conflict/partial page"}
                } else require(busy["calendars"]!!.jsonObject[review.container]!!.jsonObject.array("busy").isEmpty()){"Conflict; review another time"}
                grant.check(review,account,now())
                if(m is ProviderMutation.UpdateEvent)call(true,listOf("calendars",review.container,"events",m.event_id),"PATCH",event.body(),mapOf("sendUpdates" to "all"),m.etag,mutationAuthorization=authorizeMutation)
                else call(true,listOf("calendars",review.container,"events"),"POST",JsonObject(event.body()+mapOf("id" to JsonPrimitive(review.eventId()))),mapOf("sendUpdates" to "all"),mutationAuthorization=authorizeMutation)
            }
            is ProviderMutation.CancelEvent -> {
                scope("https://www.googleapis.com/auth/calendar.events");val old=event(review.container,m.event_id)
                require(old.string("etag")==m.etag && old.string("summary")==m.event.summary && old["start"]!!.jsonObject.string("dateTime")==m.event.start && old["end"]!!.jsonObject.string("dateTime")==m.event.end && old.array("attendees").map{it.jsonObject.string("email")}.sorted()==m.event.attendees.sorted()){"Cancellation exact event changed"}
                call(true,listOf("calendars",review.container,"events",m.event_id),"PATCH",buildJsonObject {put("status","cancelled")},mapOf("sendUpdates" to "all"),m.etag,mutationAuthorization=authorizeMutation)
            }
        }
        return verify(review,value.string("id"))
    }
    fun verify(review: Review,id: String): ProviderReceipt {
        require(review.account==account);identifier(id)
        val observed=when(review.mutation) {is ProviderMutation.SaveDraft->call(false,listOf("drafts",id),query=mapOf("format" to "raw"));is ProviderMutation.Send,is ProviderMutation.Label->call(false,listOf("messages",id),query=mapOf("format" to "raw"));else->event(review.container,id)}
        require(observed.string("id")==id){"Readback ID mismatch"}
        when(val m=review.mutation) {
            is ProviderMutation.Send,is ProviderMutation.SaveDraft -> {
                val draft=if(m is ProviderMutation.Send)m.draft else(m as ProviderMutation.SaveDraft).draft
                val message=if(m is ProviderMutation.SaveDraft)observed["message"]!!.jsonObject else observed
                if(m is ProviderMutation.Send)require(message.array("labelIds").any {it.jsonPrimitive.content=="SENT"})
                val decoder=Base64.getUrlDecoder();val actual=decoder.decode(message.string("raw")).toString(Charsets.UTF_8);val expected=decoder.decode(draft.raw(account,review.operation_id)).toString(Charsets.UTF_8)
                require(actual.endsWith(expected)){"Content/recipient normalization requires manual reconciliation"};draft.reply?.let {require(message.string("threadId")==it.thread_id)}
            }
            is ProviderMutation.Label -> {val labels=observed.array("labelIds").map{it.jsonPrimitive.content};require(id==m.message_id && m.add.all {it in labels} && m.remove.none {it in labels})}
            is ProviderMutation.CreateEvent,is ProviderMutation.UpdateEvent -> {
                val event=if(m is ProviderMutation.CreateEvent)m.event else(m as ProviderMutation.UpdateEvent).event
                require(id==(if(m is ProviderMutation.UpdateEvent)m.event_id else review.eventId()) && observed.string("status")=="confirmed" && observed.string("summary")==event.summary)
                listOf("start" to event.start,"end" to event.end).forEach{(key,time)->val date=observed[key]!!.jsonObject;require(java.time.OffsetDateTime.parse(date.string("dateTime")).toInstant()==java.time.OffsetDateTime.parse(time).toInstant() && date.string("timeZone")==event.time_zone)}
                require(observed.array("attendees").map{it.jsonObject.string("email")}.sorted()==event.attendees.sorted())
            }
            is ProviderMutation.CancelEvent -> require(id==m.event_id && observed.string("status")=="cancelled")
        }
        return ProviderReceipt(review.operation_id,review.task_id,account,id,review.container,CommitStatus.VERIFIED,now(),"Provider ID and exact postcondition read back, not just HTTP 200")
    }
}
