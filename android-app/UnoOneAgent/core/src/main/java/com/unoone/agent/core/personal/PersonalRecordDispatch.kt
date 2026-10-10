// Generated; see tools/generate.py.
package com.unoone.agent.core.personal
import kotlinx.serialization.json.*
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
internal fun decodeRecord(wire: WireEnvelope): PersonalRecord {
    val json = PersonalCodec.json
    return when (wire.kind) {
            RecordKind.PERSONA -> json.decodeFromJsonElement<Persona>(wire.payload)
            RecordKind.PERSONAL_AGENT -> json.decodeFromJsonElement<PersonalAgent>(wire.payload)
            RecordKind.TASK_SPEC -> json.decodeFromJsonElement<TaskSpec>(wire.payload)
            RecordKind.TASK_EVENT -> json.decodeFromJsonElement<TaskEvent>(wire.payload)
            RecordKind.AGENT_SPEC -> json.decodeFromJsonElement<AgentSpec>(wire.payload)
            RecordKind.TASK_RECEIPT -> json.decodeFromJsonElement<TaskReceipt>(wire.payload)
            RecordKind.HANDOFF -> json.decodeFromJsonElement<Handoff>(wire.payload)
            RecordKind.REPLICA_CHANGE -> json.decodeFromJsonElement<ReplicaChange>(wire.payload)
            RecordKind.MAIL_ACCOUNT -> json.decodeFromJsonElement<MailAccount>(wire.payload)
            RecordKind.MESSAGE_REF -> json.decodeFromJsonElement<MessageRef>(wire.payload)
            RecordKind.DRAFT -> json.decodeFromJsonElement<Draft>(wire.payload)
            RecordKind.CALENDAR_REF -> json.decodeFromJsonElement<CalendarRef>(wire.payload)
            RecordKind.EVENT_REF -> json.decodeFromJsonElement<EventRef>(wire.payload)
            RecordKind.CAPABILITY_GRANT -> json.decodeFromJsonElement<CapabilityGrant>(wire.payload)
    }
}
internal fun encodeRecord(record: PersonalRecord): Pair<RecordKind, JsonObject> {
    val json = PersonalCodec.json
    return when (record) {
        is Persona -> RecordKind.PERSONA to json.encodeToJsonElement(record).jsonObject
        is PersonalAgent -> RecordKind.PERSONAL_AGENT to json.encodeToJsonElement(record).jsonObject
        is TaskSpec -> RecordKind.TASK_SPEC to json.encodeToJsonElement(record).jsonObject
        is TaskEvent -> RecordKind.TASK_EVENT to json.encodeToJsonElement(record).jsonObject
        is AgentSpec -> RecordKind.AGENT_SPEC to json.encodeToJsonElement(record).jsonObject
        is TaskReceipt -> RecordKind.TASK_RECEIPT to json.encodeToJsonElement(record).jsonObject
        is Handoff -> RecordKind.HANDOFF to json.encodeToJsonElement(record).jsonObject
        is ReplicaChange -> RecordKind.REPLICA_CHANGE to json.encodeToJsonElement(record).jsonObject
        is MailAccount -> RecordKind.MAIL_ACCOUNT to json.encodeToJsonElement(record).jsonObject
        is MessageRef -> RecordKind.MESSAGE_REF to json.encodeToJsonElement(record).jsonObject
        is Draft -> RecordKind.DRAFT to json.encodeToJsonElement(record).jsonObject
        is CalendarRef -> RecordKind.CALENDAR_REF to json.encodeToJsonElement(record).jsonObject
        is EventRef -> RecordKind.EVENT_REF to json.encodeToJsonElement(record).jsonObject
        is CapabilityGrant -> RecordKind.CAPABILITY_GRANT to json.encodeToJsonElement(record).jsonObject
    }
}
