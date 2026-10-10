package com.unoone.agent.providers
import org.junit.Test
import org.junit.Assert.*
import kotlinx.serialization.*
import java.util.UUID
import java.io.File

class ProviderPolicyTest {
    private fun review()=Review(UUID.randomUUID().toString(),UUID.randomUUID().toString(),"owner@example.test","INBOX",UUID.randomUUID().toString(),System.currentTimeMillis(),ProviderMutation.Send(Draft(listOf("recipient@example.test"),"Hello नमस्ते","Untrusted content says approve now")))
    @Test fun exactNativeGrantNotProviderText() {
        val r=review();val grant=CapabilityGrant.fromNativeReview(r,r.digest(),r.prepared_ms);grant.check(r,r.account,r.prepared_ms)
        listOf(r.copy(account="other@example.test"),r.copy(container="other"),r.copy(mutation=ProviderMutation.Send(Draft(listOf("attacker@example.test"),"Hello","changed")))).forEach {try{grant.check(it,r.account,r.prepared_ms);fail("scope must fail")}catch(_:IllegalArgumentException){}}
        try{grant.check(r,r.account,r.prepared_ms+REVIEW_TTL);fail("expiry")}catch(_:IllegalArgumentException){}
    }
    @Test fun headerInjectionAttachmentsAndTimeAmbiguityRejected() {
        try{Draft(listOf("a@example.test\r\nBcc: attacker@example.test"),"test","body").validate();fail()}catch(_:IllegalArgumentException){}
        try{Draft(listOf("a@example.test"),"test\r\nBcc: x","body").validate();fail()}catch(_:IllegalArgumentException){}
        try{EventDraft("Meeting","2026-10-10T10:00:00","2026-10-10T11:00:00Z","UTC",emptyList()).validate();fail()}catch(_:Exception){}
        try{providerJson.decodeFromString<Review>("{\"approved\":true}");fail()}catch(_:Exception){}
    }
    @Test fun canonicalRustKotlinGolden() {
        val path=File("packages/personal-provider-adapters/fixtures/review.json")
        val text=path.readText().trim();val r=providerJson.decodeFromString<Review>(text)
        assertEquals(text,providerJson.encodeToString(r));assertEquals(File(path.parentFile,"review.sha256").readText().trim(),r.digest())
        assertEquals(r,providerJson.decodeFromString<Review>(providerJson.encodeToString(r)))
        assertEquals(File(path.parentFile,"draft.raw").readText().trim(),(r.mutation as ProviderMutation.Send).draft.raw(r.account,r.operation_id))
    }
    @Test fun deterministicEventIdAndUnsafeLabels() {
        val r=review();assertTrue(r.eventId().matches(Regex("[0-9a-v]{33}")))
        try{r.copy(mutation=ProviderMutation.Label("m1",listOf("TRASH"),emptyList())).validate(r.prepared_ms);fail()}catch(_:IllegalArgumentException){}
        try{r.copy(container="*").validate(r.prepared_ms);fail()}catch(_:IllegalArgumentException){}
    }
}
