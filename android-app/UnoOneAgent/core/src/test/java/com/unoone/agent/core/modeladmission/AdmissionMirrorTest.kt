package com.unoone.agent.core.modeladmission

import java.io.File
import kotlinx.serialization.json.*
import org.junit.Test
import org.junit.Assert.*

class AdmissionMirrorTest {
    private fun fixture(): JsonObject {
        val file = System.getProperty("model.admission.fixture")?.let(::File)
            ?: generateSequence(File(System.getProperty("user.dir")).absoluteFile) { it.parentFile }
                .map { File(it, "packages/model-admission/tests/fixtures/admission-v1.json") }.first { it.isFile }
        return Json.parseToJsonElement(file.readText()).jsonObject
    }
    private fun replace(node: JsonElement, path: List<String>, value: JsonElement): JsonElement {
        if (path.isEmpty()) return value
        return when (node) {
            is JsonObject -> JsonObject(node.toMutableMap().apply { put(path.first(), replace(getValue(path.first()), path.drop(1), value)) })
            is JsonArray -> JsonArray(node.toMutableList().apply { val i = path.first().toInt(); set(i, replace(get(i), path.drop(1), value)) })
            else -> error("Bad shared fixture path")
        }
    }
    @Test fun exactSharedGoldenJsonCases() {
        val base = fixture()
        val cases = base.getValue("cases").jsonArray
        assertEquals(14, cases.size)
        for (row in cases) {
            val case = row.jsonObject; var data: JsonElement = base
            case.getValue("changes").jsonObject.forEach { (path, value) -> data = replace(data, path.split('.'), value) }
            val v = data.jsonObject
            val c = v.getValue("candidate").jsonObject
            val q = replace(v.getValue("qualification"), listOf("scope", "candidate"), c).jsonObject
            val d = AdmissionMirror.evaluate(c, v.getValue("probe").jsonObject, v.getValue("request").jsonObject,
                listOf(q), v.getValue("now_ms").jsonPrimitive.long)
            val name = case.getValue("name").jsonPrimitive.content
            assertEquals(name, case.getValue("status").jsonPrimitive.content, d.status)
            assertEquals(name, case.getValue("reason").jsonPrimitive.content, d.reason)
            assertEquals(name, case.getValue("eligible").jsonPrimitive.boolean, d.eligibleNow)
        }
    }
    @Test fun untrustedQualificationNeverEntersPublicOrdinaryLane() {
        val v = fixture()
        val d = AdmissionMirror.assessUnqualified(v.getValue("candidate").jsonObject, v.getValue("probe").jsonObject,
            v.getValue("request").jsonObject, 100000)
        assertEquals("NOT_YET_QUALIFIED", d.status)
        assertFalse(d.eligibleNow)
        assertEquals("UNQUALIFIED", d.reason)
    }
    @Test fun staleProbeDoesNotBecomeFreshOnReplay() {
        val v = fixture()
        assertEquals("STALE_PROBE", AdmissionMirror.assessUnqualified(v.getValue("candidate").jsonObject,
            v.getValue("probe").jsonObject, v.getValue("request").jsonObject, 160001).reason)
    }
}
