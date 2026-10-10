package com.unoone.agent.execution

import org.junit.Assert.*
import org.junit.Test
import java.time.Instant
import java.time.ZoneId

class CalendarIntervalPolicyTest {
    private val ny = ZoneId.of("America/New_York")
    @Test fun splitFieldsResolveExactFutureInterval() {
        val value = CalendarIntervalPolicy.resolve("2031-07-22", "15:00", "16:00", ny)
        assertEquals(Instant.parse("2031-07-22T19:00:00Z"), value.start)
        assertEquals(Instant.parse("2031-07-22T20:00:00Z"), value.end)
        assertTrue(value.report().contains("timezone=America/New_York"))
    }
    @Test fun fullInstantAndZonedFormsRemainSupported() {
        assertEquals(Instant.parse("2031-07-22T15:00:00Z"), CalendarIntervalPolicy.resolve(null,
            "2031-07-22T15:00:00Z", "2031-07-22T16:00:00Z").start)
        assertEquals(ny, CalendarIntervalPolicy.resolve(null,
            "2031-07-22T15:00:00-04:00[America/New_York]", "2031-07-22T16:00:00-04:00[America/New_York]").zone)
        assertEquals(ny, CalendarIntervalPolicy.resolve(null,
            "2031-07-22T15:00:00", "2031-07-22T16:00:00", ny).zone)
    }
    @Test fun missingInvalidAndNonpositiveIntervalsNeedUser() {
        listOf(
            Triple(null, "15:00", "16:00"), Triple("2031-07-22", null, "16:00"),
            Triple("2031-07-22", "15:00", null), Triple("2031-07-22", null, null),
            Triple("2031-02-29", "15:00", "16:00"), Triple("2031-07-22", "24:00", "25:00"),
            Triple("2031-07-22", "16:00", "15:00"), Triple("2031-07-22", "15:00", "15:00"),
            Triple("tomorrow", "15:00", "16:00"), Triple("2031-07-22", "2031-07-23T15:00:00Z", "2031-07-23T16:00:00Z")
        ).forEach { (date,start,end) -> needsUser { CalendarIntervalPolicy.resolve(date,start,end,ny) } }
        needsUser { CalendarIntervalPolicy.resolve("2031-07-22","15:00","16:00") }
        needsUser { CalendarIntervalPolicy.resolve(null,"2031-07-22T15:00:00","2031-07-22T16:00:00") }
    }
    @Test fun dstGapAndUnqualifiedOverlapNeverNormalize() {
        needsUser { CalendarIntervalPolicy.resolve("2026-03-08","02:15","03:15",ny) }
        needsUser { CalendarIntervalPolicy.resolve("2026-11-01","01:15","01:45",ny) }
        needsUser { CalendarIntervalPolicy.resolve(null,"2026-03-08T02:15:00-05:00[America/New_York]","2026-03-08T03:15:00-04:00[America/New_York]") }
        needsUser { CalendarIntervalPolicy.resolve(null,"2026-07-22T15:00:00-05:00[America/New_York]","2026-07-22T16:00:00-05:00[America/New_York]") }
        val explicit = CalendarIntervalPolicy.resolve(null,"2026-11-01T01:15:00-04:00[America/New_York]","2026-11-01T01:45:00-05:00[America/New_York]")
        assertEquals(90*60*1000L, explicit.endMs-explicit.startMs)
    }
    private fun needsUser(block: () -> Any) {
        try { block(); fail("Expected NEEDS_USER") }
        catch(e: IllegalArgumentException) { assertTrue(e.message.orEmpty().contains("NEEDS_USER")) }
    }
}
