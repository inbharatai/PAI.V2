package com.unoone.agent.execution

import android.content.Context
import com.unoone.agent.accessibilitycontrol.AccessibilityControl
import com.unoone.agent.agentrouter.AgentRouter
import com.unoone.agent.core.model.*
import com.unoone.agent.core.runtime.AgentRuntimeGate
import com.unoone.agent.phonecontrol.*
import com.unoone.agent.task.NativeTaskFixture
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.lang.reflect.Proxy
import java.time.Instant
import java.time.ZoneId

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE, application = android.app.Application::class, shadows = [CalendarHostOsShadow::class])
class CalendarBoundaryExecutionTest {
    private class ControlledCalendarAdapter : CalendarAdapter {
        val queries = mutableListOf<Pair<Long,Long>>()
        val busyStart = Instant.parse("2031-07-22T19:00:00Z").toEpochMilli()
        val busyEnd = Instant.parse("2031-07-22T20:00:00Z").toEpochMilli()
        var unavailable = false
        override fun getEvents(startMs: Long, endMs: Long): Result<List<CalendarControl.CalendarEvent>> {
            queries += startMs to endMs
            if (unavailable) return Result.Error("permission denied")
            // Current time has no events. Only querying the actual requested future range finds this.
            return Result.Success(if(startMs < busyEnd && endMs > busyStart)
                listOf(CalendarControl.CalendarEvent("Future appointment",busyStart,busyEnd,"")) else emptyList())
        }
    }
    private inline fun <reified T> unusedDao(): T = Proxy.newProxyInstance(T::class.java.classLoader,
        arrayOf(T::class.java)) { _,method,_ -> error("Calendar must not access DAO ${method.name}") } as T
    private fun executor(context: Context, adapter: CalendarAdapter, zone: ZoneId? = ZoneId.of("America/New_York")) = ActionExecutor(
        context, unusedDao(), unusedDao(), unusedDao(), unusedDao(), PhoneControl(context), CalendarControl(context),
        OcrControl(context), AccessibilityControl(), AgentRouter(), adapter, zone)
    private fun call(date: String? = "2031-07-22", start: String? = "15:00", end: String? = "16:00", tool: String = "check_calendar_conflict") = ToolCall(tool, buildJsonObject {
        if (tool != "check_calendar_conflict") put("title","Appointment")
        date?.let { put("date",it) }; start?.let { put("start_time",it) }; end?.let { put("end_time",it) }
    })
    @Test fun actualExecutorQueriesFutureSlotNotEmptyNowAndReportsResolvedTimezone() = runBlocking {
        NativeTaskFixture().use { fixture ->
            AgentRuntimeGate.setEnabled(true)
            val calendar = ControlledCalendarAdapter(); val executor = executor(fixture.context,calendar)
            val call = call()
            assertNull(ToolCallValidator.rejection(call)) // exactly the existing advertised split schema
            val result = fixture.tool(call) { executor.executeTool(call) }
            assertTrue(result is Result.Success)
            val report = (result as Result.Success).data
            assertTrue(report, report.contains("Found 1 conflict"))
            assertTrue(report, report.contains("Future appointment"))
            assertTrue(report, report.contains("timezone=America/New_York"))
            assertTrue(report, report.contains("2031-07-22T19:00:00Z/2031-07-22T20:00:00Z"))
            assertEquals(listOf(calendar.busyStart to calendar.busyEnd), calendar.queries)
        }
    }
    @Test fun unavailableCalendarNeverReportsFree() = runBlocking {
        NativeTaskFixture().use { f ->
            AgentRuntimeGate.setEnabled(true)
            val calendar = ControlledCalendarAdapter().also { it.unavailable = true }
            val e = executor(f.context,calendar); val c = call()
            val result = f.tool(c) { e.executeTool(c) }
            assertTrue(result is Result.Error)
            assertTrue((result as Result.Error).message.contains("availability unknown"))
        }
    }
    @Test fun invalidInputUnknownZoneAndDstNeverQueryOrLaunch() = runBlocking {
        NativeTaskFixture().use { f ->
            AgentRuntimeGate.setEnabled(true)
            val calendar = ControlledCalendarAdapter()
            val e = executor(f.context,calendar)
            val invalid = listOf(call(end=null),call(start=null,end=null),call(start="bad"),call(end="14:00"),
                call("2026-03-08","02:15","03:15"),call("2026-11-01","01:15","01:45"),
                call(start="bad",tool="create_calendar_event"),call(null,null,null,"open_calendar_insert"))
            for(c in invalid) {
                val result = f.tool(c) { e.executeTool(c) }
                assertTrue("$c -> $result", result is Result.Error)
                assertTrue((result as Result.Error).message, result.message.contains("NEEDS_USER"))
            }
            val c = call(); val unknownZone = executor(f.context,calendar,null)
            val result = f.tool(c) { unknownZone.executeTool(c) }
            assertTrue(result is Result.Error && result.message.contains("NEEDS_USER"))
            assertTrue(calendar.queries.isEmpty())
            assertNull(org.robolectric.Shadows.shadowOf(f.context as android.content.ContextWrapper).nextStartedActivity)
        }
    }
    @Test fun nativeAdapterNullCursorIsUnknownNotFree() {
        val context = org.robolectric.RuntimeEnvironment.getApplication()
        val provider = object : android.content.ContentProvider() {
            override fun onCreate() = true
            override fun query(uri: android.net.Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): android.database.Cursor? = null
            override fun getType(uri: android.net.Uri): String? = null
            override fun insert(uri: android.net.Uri, values: android.content.ContentValues?): android.net.Uri? = error("No writes")
            override fun delete(uri: android.net.Uri, selection: String?, selectionArgs: Array<out String>?): Int = error("No writes")
            override fun update(uri: android.net.Uri, values: android.content.ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = error("No writes")
        }
        org.robolectric.shadows.ShadowContentResolver.registerProviderInternal("com.android.calendar",provider)
        assertTrue(AndroidCalendarAdapter(context).getEvents(1000,2000) is Result.Error)
    }
    @Test fun composerLaunchIsOnlyActionVerifiedNeverPersistenceOrSend() = runBlocking {
        NativeTaskFixture().use { f ->
            AgentRuntimeGate.setEnabled(true)
            val intent = android.content.Intent(android.content.Intent.ACTION_INSERT)
                .setDataAndType(android.provider.CalendarContract.Events.CONTENT_URI,"vnd.android.cursor.dir/event")
            val info = android.content.pm.ResolveInfo().apply {
                activityInfo = android.content.pm.ActivityInfo().apply { packageName="calendar.test";name="CalendarActivity" }
            }
            org.robolectric.Shadows.shadowOf(f.context.packageManager).addResolveInfoForIntent(intent,info)
            val service = org.robolectric.Robolectric.buildService(com.unoone.agent.accessibilitycontrol.UnoOneAccessibilityService::class.java).create().get()
            org.robolectric.util.ReflectionHelpers.callInstanceMethod<Unit>(service,"onServiceConnected")
            val event = android.view.accessibility.AccessibilityEvent.obtain(android.view.accessibility.AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED)
            event.packageName="calendar.test";service.onAccessibilityEvent(event)
            try {
                val calendar=ControlledCalendarAdapter();val e=executor(f.context,calendar)
                for(tool in listOf("create_calendar_event","open_calendar_insert")) {
                    val c = if(tool=="create_calendar_event") call(tool=tool)
                        else call(null,"2031-07-22T19:00:00Z","2031-07-22T20:00:00Z",tool)
                    val result=f.tool(c) { e.executeTool(c) }
                    assertTrue("$result",result is Result.Success)
                    val report=(result as Result.Success).data
                    assertTrue(report,report.startsWith("ACTION_VERIFIED:"))
                    assertTrue(report,report.contains("no event persistence or invitation delivery was verified"))
                    val launched=org.robolectric.Shadows.shadowOf(f.context as android.content.ContextWrapper).nextStartedActivity
                    assertEquals(android.content.Intent.ACTION_INSERT,launched.action)
                    assertEquals(calendar.busyStart,launched.getLongExtra(android.provider.CalendarContract.EXTRA_EVENT_BEGIN_TIME,-1))
                    assertEquals(calendar.busyEnd,launched.getLongExtra(android.provider.CalendarContract.EXTRA_EVENT_END_TIME,-1))
                }
                assertTrue(calendar.queries.isEmpty())
            } finally { service.onDestroy();event.recycle() }
        }
    }
    @Test fun fullInstantGoesThroughActualExecutorWithNoDefaultZone() = runBlocking {
        NativeTaskFixture().use { f ->
            AgentRuntimeGate.setEnabled(true)
            val calendar = ControlledCalendarAdapter(); val e = executor(f.context,calendar,null)
            val c = call(null,"2031-07-22T19:00:00Z","2031-07-22T20:00:00Z")
            val result = f.tool(c) { e.executeTool(c) }
            assertTrue(result is Result.Success && result.data.contains("Found 1 conflict"))
            assertEquals(listOf(calendar.busyStart to calendar.busyEnd),calendar.queries)
        }
    }
}
