package com.unoone.agent.execution

import java.time.*
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeParseException

/** No wall-clock or device-zone fallback. The optional zone must come from known native user
 * configuration, never an inferred locale. Full ISO offset/zone forms carry their own authority. */
object CalendarIntervalPolicy {
    data class Interval(val start: Instant, val end: Instant, val zone: ZoneId) {
        val startMs: Long get() = start.toEpochMilli()
        val endMs: Long get() = end.toEpochMilli()
        fun report() = "${start.atZone(zone)} to ${end.atZone(zone)}; timezone=${zone.id}; UTC=$start/$end"
    }

    fun resolve(date: String?, start: String?, end: String?, configuredZone: ZoneId? = null): Interval {
        try {
            require(!start.isNullOrBlank() && !end.isNullOrBlank()) { "Both start and end are required" }
            val day = date?.let {
                require(it.matches(Regex("\\d{4}-\\d{2}-\\d{2}"))) { "Date must be YYYY-MM-DD" }
                LocalDate.parse(it)
            }
            fun endpoint(value: String): ZonedDateTime {
                if (value.matches(Regex("\\d{2}:\\d{2}"))) {
                    require(day != null) { "Time-only input requires a date" }
                    return strictLocal(day.atTime(LocalTime.parse(value)),
                        requireNotNull(configuredZone) { "A known user-configured timezone is required" })
                }
                // Read the original local/offset fields, not ZonedDateTime.parse's DST normalization.
                val parsed = try { DateTimeFormatter.ISO_ZONED_DATE_TIME.parse(value) }
                    catch (_: DateTimeParseException) { null }
                if (parsed != null) {
                    val local = LocalDateTime.from(parsed)
                    val zone = ZoneId.from(parsed)
                    val offset = ZoneOffset.from(parsed)
                    require(offset in zone.rules.getValidOffsets(local)) { "Offset is invalid in timezone (DST gap or mismatch)" }
                    require(day == null || day == local.toLocalDate()) { "Date conflicts with full timestamp" }
                    // An explicit valid offset disambiguates a repeated time; a local overlap never does.
                    return ZonedDateTime.ofStrict(local, offset, zone)
                }
                val local = LocalDateTime.parse(value)
                require(day == null || day == local.toLocalDate()) { "Date conflicts with full timestamp" }
                return strictLocal(local, requireNotNull(configuredZone) { "A known user-configured timezone is required" })
            }
            val a = endpoint(start)
            val b = endpoint(end)
            require(a.zone == b.zone) { "Use the same timezone for both endpoints" }
            require(b.toInstant().isAfter(a.toInstant())) { "End must be after start" }
            val interval = Interval(a.toInstant(), b.toInstant(), a.zone)
            require(interval.endMs > interval.startMs) { "Interval must be positive at calendar millisecond precision" }
            return interval
        } catch (e: RuntimeException) {
            throw IllegalArgumentException("NEEDS_USER: Calendar interval unresolved: ${e.message}. Supply exact start/end and timezone.", e)
        }
    }

    private fun strictLocal(local: LocalDateTime, zone: ZoneId): ZonedDateTime {
        val offsets = zone.rules.getValidOffsets(local)
        require(offsets.size == 1) { "DST gap or ambiguous local time; supply an explicit valid offset" }
        return ZonedDateTime.ofStrict(local, offsets.single(), zone)
    }
}
