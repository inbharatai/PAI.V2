package com.unoone.agent.execution

import com.unoone.agent.core.model.Result
import com.unoone.agent.phonecontrol.CalendarControl

/** Bounded calendar read seam; tests exercise the same query/report path as ActionExecutor. */
fun interface CalendarAdapter {
    fun getEvents(startMs: Long, endMs: Long): Result<List<CalendarControl.CalendarEvent>>
}

/** A null/failed/truncated cursor is unknown, not a successful empty calendar. Kept at the
 * execution boundary so the legacy CalendarControl and its other callers need not change. */
internal class AndroidCalendarAdapter(private val context: android.content.Context) : CalendarAdapter {
    override fun getEvents(startMs: Long, endMs: Long): Result<List<CalendarControl.CalendarEvent>> = try {
        require(endMs > startMs)
        val uri = android.provider.CalendarContract.Instances.CONTENT_URI.buildUpon().apply {
            android.content.ContentUris.appendId(this, startMs)
            android.content.ContentUris.appendId(this, endMs)
        }.build()
        val projection = arrayOf(android.provider.CalendarContract.Instances.TITLE,
            android.provider.CalendarContract.Instances.BEGIN, android.provider.CalendarContract.Instances.END,
            android.provider.CalendarContract.Instances.EVENT_LOCATION)
        val cursor = checkNotNull(context.contentResolver.query(uri, projection, null, null, null)) { "Calendar query unavailable" }
        val events = cursor.use {
            val title = it.getColumnIndexOrThrow(projection[0])
            val begin = it.getColumnIndexOrThrow(projection[1])
            val end = it.getColumnIndexOrThrow(projection[2])
            val location = it.getColumnIndexOrThrow(projection[3])
            val rows = mutableListOf<CalendarControl.CalendarEvent>()
            while (it.moveToNext()) {
                check(rows.size < 10_000) { "Calendar result limit exceeded; availability unknown" }
                check(!it.isNull(begin) && !it.isNull(end)) { "Calendar event time missing" }
                val a = it.getLong(begin); val b = it.getLong(end)
                check(b > a) { "Calendar event interval invalid" }
                rows += CalendarControl.CalendarEvent(it.getString(title).orEmpty(), a, b, it.getString(location).orEmpty())
            }
            rows
        }
        Result.Success(events)
    } catch (e: Exception) { Result.Error("Calendar availability unknown: ${e.message}") }
}

internal object CalendarConflictQuery {
    fun execute(interval: CalendarIntervalPolicy.Interval, calendar: CalendarAdapter): Result<String> {
        val report = interval.report()
        return when (val result = calendar.getEvents(interval.startMs, interval.endMs)) {
            is Result.Error -> Result.Error("Calendar availability unknown for $report: ${result.message}")
            is Result.Success -> {
                val conflicts = result.data.filter { it.startTime < interval.endMs && it.endTime > interval.startMs }
                if (conflicts.isEmpty()) Result.Success("No calendar conflicts found for $report.")
                else Result.Success("Found ${conflicts.size} conflict(s) for $report: " +
                    conflicts.take(5).joinToString("; ") {
                        "${it.title} at ${java.time.Instant.ofEpochMilli(it.startTime).atZone(interval.zone)}"
                    })
            }
        }
    }
}
