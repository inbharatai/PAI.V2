#!/usr/bin/env python3
"""Generate DTO declarations, strict JSON schema and shared examples. No dependencies.
Run from any directory; semantic validation/native authority are hand-written.
"""
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parents[3]
PKG = ROOT / 'packages/personal-agent-contracts'
KOTLIN = ROOT / 'android-app/UnoOneAgent/core/src/main/java/com/unoone/agent/core/personal'
ENUMS = {
'Sensitivity':['PUBLIC','PRIVATE','SENSITIVE'],
'PreferenceStatus':['OBSERVED','APPROVED','CORRECTED'],
'ProvenanceSource':['USER','MODEL','NATIVE','PEER'],
'HostKind':['ANDROID','DESKTOP'],
'DelegationLevel':['READ_AND_SUGGEST','PREPARE_DRAFTS','ACT_WITHIN_SCOPE'],
'NetworkPolicy':['OFFLINE_ONLY','EXISTING_PROVIDER'],
'TaskTransition':['PLANNED','WAITING_FOR_ACCESS','DRAFTED','READY_FOR_REVIEW','IN_PROGRESS','AWAITING_VERIFICATION','VERIFIED','BLOCKED','CANCELLED'],
'ReceiptOutcome':['VERIFIED','ACTION_VERIFIED','RESPONDED','UNVERIFIED','NEEDS_USER','FAILED','CANCELLED'],
'ModelSelectionRule':['LOCAL_QUALIFIED_ONLY'],
'ApprovalState':['PENDING','APPROVED','REJECTED'],
'ChangeKind':['UPSERT','TOMBSTONE'],
'ProviderKind':['LOCAL','GMAIL','OUTLOOK','IMAP'],
'DataKind':['ACCOUNT','FOLDER','CALENDAR','FILE','APP','RECORD'],
'Operation':['READ','SUGGEST','DRAFT','ORGANIZE','SEND','CREATE','UPDATE','DELETE','INVITE'],
'DispatchIntent':['NONE','OPEN_COMPOSER','OPEN_EVENT_FORM','PROVIDER_READ','PROVIDER_MUTATION','LOCAL_MUTATION'],
}
# Optional values are explicitly null on the wire; only read-only defaults may be omitted.
TYPES = {
'Version': [('major','u64'),('minor','u64')],
'Provenance':[('source','ProvenanceSource'),('actor_id','String'),('replica_id','String'),('evidence_ref','String?')],
'Preference':[('key','String'),('value','String'),('status','PreferenceStatus'),('provenance','Provenance')],
'Budget':[('max_steps','u64'),('max_tool_calls','u64'),('max_duration_ms','u64'),('max_bytes','u64'),('max_network_calls','u64'),('max_depth','u64'),('max_children','u64')],
'DataScope':[('kind','DataKind'),('resource_id','String'),('operations','Operation[]')],
'Scopes':[('capabilities','String[]'),('tools','String[]'),('data','DataScope[]'),('recipients','String[]'),('hosts','String[]'),('operations','Operation[]'),('delegation','DelegationLevel'),('network','NetworkPolicy')],
'Persona':[('person_id','String'),('revision','u64'),('preferences','Preference[]'),('sensitivity','Sensitivity'),('provenance','Provenance'),('deleted','bool'),('corrects_revision','u64?')],
'PersonalAgent':[('agent_id','String'),('person_id','String'),('profile_revision','u64'),('display_name','String'),('persona_revision','u64'),('conversation_refs','String[]'),('capability_preferences','String[]')],
'TaskSpec':[('task_id','String'),('agent_id','String'),('person_id','String'),('idempotency_key','String'),('goal','String'),('origin_replica_id','String'),('target_replica_id','String?'),('host_requirements','HostKind[]'),('scopes','Scopes'),('budget','Budget'),('created_at_ms','u64'),('deadline_ms','u64'),('expected_postcondition','String'),('user_visible_policy','String'),('sensitivity','Sensitivity')],
'TaskEvent':[('event_id','String'),('operation_id','String'),('task_id','String'),('predecessor_event_id','String?'),('origin_replica_id','String'),('assigned_replica_id','String?'),('step','u64'),('deadline_ms','u64'),('transition','TaskTransition'),('evidence_ref','String?'),('external_object_id','String?')],
'AgentSpec':[('child_id','String'),('parent_task_id','String'),('purpose','String'),('template_version','u64'),('model_selection','ModelSelectionRule'),('scopes','Scopes'),('budget','Budget'),('depth','u64'),('expires_at_ms','u64'),('stop_generation','u64'),('verifier_id','String')],
'TaskReceipt':[('operation_id','String'),('task_id','String'),('attempt_id','String'),('actor_id','String'),('replica_id','String'),('before_evidence_refs','String[]'),('after_evidence_refs','String[]'),('dispatch_intent','DispatchIntent'),('observed_effect','String'),('outcome','ReceiptOutcome'),('reason','String'),('source','ProvenanceSource'),('timestamp_ms','u64'),('external_object_id','String?')],
'Handoff':[('handoff_id','String'),('task_id','String'),('from_replica_id','String'),('target_replica_id','String'),('encrypted_goal','String'),('receipt_operation_ids','String[]'),('host_requirements','HostKind[]'),('approval','ApprovalState'),('expires_at_ms','u64')],
'ReplicaChange':[('person_id','String'),('replica_id','String'),('sequence','u64'),('operation_id','String'),('record_id','String'),('record_kind','RecordKind'),('predecessor_operation_id','String?'),('record_revision','u64'),('change','ChangeKind'),('ciphertext','String?'),('content_hash','String'),('provenance','Provenance')],
'MailAccount':[('account_id','String'),('person_id','String'),('provider','ProviderKind'),('display_name','String'),('address','String')],
'MessageRef':[('account_id','String'),('message_id','String'),('thread_id','String?'),('folder_id','String?')],
'Draft':[('draft_id','String'),('task_id','String'),('account_id','String'),('reply_to','MessageRef?'),('recipients','String[]'),('subject','String'),('body','String'),('attachment_refs','String[]'),('revision','u64'),('approval','ApprovalState')],
'CalendarRef':[('account_id','String'),('calendar_id','String'),('display_name','String'),('time_zone','String')],
'EventRef':[('calendar','CalendarRef'),('event_id','String'),('start_ms','u64'),('end_ms','u64'),('time_zone','String'),('attendees','String[]')],
'CapabilityGrant':[('grant_id','String'),('person_id','String'),('replica_id','String'),('scopes','Scopes'),('budget','Budget'),('issued_at_ms','u64'),('expires_at_ms','u64'),('stop_generation','u64'),('revoked','bool')],
}
RECORDS = list(TYPES)[6:]
assert RECORDS[0] == 'Persona'
def snake(s):
    import re
    return re.sub(r'(?<!^)(?=[A-Z])','_',s).lower()
def upper(s): return snake(s).upper()
def rust_variant(s): return ''.join(x.title() for x in s.split('_'))
ENUMS['RecordKind'] = [upper(r) for r in RECORDS]
DEFAULTS = {'delegation':('DelegationLevel::ReadAndSuggest','DelegationLevel.READ_AND_SUGGEST','READ_AND_SUGGEST'), 'network':('NetworkPolicy::OfflineOnly','NetworkPolicy.OFFLINE_ONLY','OFFLINE_ONLY')}
def rtype(t):
    if t.endswith('?'): return 'Option<'+rtype(t[:-1])+'>'
    if t.endswith('[]'): return 'Vec<'+rtype(t[:-2])+'>'
    return t
def ktype(t):
    if t.endswith('?'): return ktype(t[:-1])+'?'
    if t.endswith('[]'): return 'List<'+ktype(t[:-2])+'>'
    return {'u64':'Long','bool':'Boolean'}.get(t,t)
rust = ['// Generated by tools/generate.py; edit the definition there.', 'use serde::{Deserialize, Serialize};']
kt = ['// Generated by packages/personal-agent-contracts/tools/generate.py.', 'package com.unoone.agent.core.personal', '', 'import kotlinx.serialization.Serializable', 'import kotlinx.serialization.SerialName', '', 'sealed interface PersonalRecord']
for name, values in ENUMS.items():
    default = name in ('DelegationLevel', 'NetworkPolicy')
    derive = '#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord' + (', Default' if default else '') + ')]'
    rust += [derive, '#[serde(rename_all = "SCREAMING_SNAKE_CASE")]',f'pub enum {name} {{ '+', '.join(('#[default] ' if default and i == 0 else '') + rust_variant(v) for i,v in enumerate(values))+' }']
    kt += ['@Serializable',f'enum class {name} {{ '+', '.join(values)+' }']
for name,fields in TYPES.items():
    rust += ['#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]', '#[serde(deny_unknown_fields)]',f'pub struct {name} {{']
    kt += ['', '@Serializable', f'data class {name}(']
    for field,t in fields:
        if field in DEFAULTS: rust += ['    #[serde(default)]']
        if t.endswith('?'): rust += ['    #[serde(deserialize_with = "crate::required_nullable")]']
        rust += [f'    pub {field}: {rtype(t)},']
        default = ' = '+DEFAULTS[field][1] if field in DEFAULTS else ''
        kt += [f'    val {field}: {ktype(t)}{default},']
    rust += ['}']
    kt += [')'+(' : PersonalRecord' if name in RECORDS else '')]
rust += ['#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]', '#[serde(tag = "kind", content = "payload", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]', 'pub enum Record {']
rust += [f'    {r}({r}),' for r in RECORDS] + ['}']
(PKG/'src/types.rs').write_text('\n'.join(rust)+'\n')
(KOTLIN/'PersonalDtos.kt').write_text('\n'.join(kt)+'\n')
# Strict schema: generic boundary limits are additionally enforced in both codecs.
def schema_type(t):
    if t.endswith('?'): return {'anyOf':[schema_type(t[:-1]),{'type':'null'}]}
    if t.endswith('[]'): return {'type':'array','maxItems':64,'items':schema_type(t[:-2])}
    if t == 'String': return {'type':'string','maxLength':4096}
    if t == 'bool': return {'type':'boolean'}
    if t == 'u64': return {'type':'integer','minimum':0,'maximum':9007199254740991}
    return {'$ref':'#/$defs/'+t}
defs = {n:{'type':'string','enum':v} for n,v in ENUMS.items()}
for name,fields in TYPES.items():
    props={f:schema_type(t) for f,t in fields}
    for f in props:
        if f in DEFAULTS: props[f]['default']=DEFAULTS[f][2]
    defs[name]={'type':'object','additionalProperties':False,'required':[f for f,_ in fields if f not in DEFAULTS], 'properties':props}
schema={'$schema':'https://json-schema.org/draft/2020-12/schema','$id':'inbharat.pai.personal-agent.v1','title':'Inert personal-agent records (not authority)','oneOf':[], '$defs':defs}
for r in RECORDS:
    schema['oneOf'].append({'type':'object','additionalProperties':False,'required':['schema','version','kind','payload'],'properties':{'schema':{'const':'inbharat.pai.personal-agent'},'version':{'type':'object','additionalProperties':False,'required':['major','minor'],'properties':{'major':{'const':1},'minor':{'const':0}}},'kind':{'const':upper(r)},'payload':{'$ref':'#/$defs/'+r}}})
(PKG/'personal-agent.v1.schema.json').write_text(json.dumps(schema,indent=2)+'\n')
# Shared synthetic records only: no real data or secrets.
p={'source':'USER','actor_id':'person-1','replica_id':'phone-1','evidence_ref':None}
b={'max_steps':8,'max_tool_calls':8,'max_duration_ms':60000,'max_bytes':8192,'max_network_calls':0,'max_depth':2,'max_children':2}
s={'capabilities':['mail.read'],'tools':['draft_email'],'data':[{'kind':'ACCOUNT','resource_id':'mail-1','operations':['READ']}],'recipients':[],'hosts':['phone-1'],'operations':['READ','SUGGEST'],'delegation':'READ_AND_SUGGEST','network':'OFFLINE_ONLY'}
calendar={'account_id':'mail-1','calendar_id':'cal-1','display_name':'Personal','time_zone':'Asia/Kolkata'}
examples={
'Persona':dict(person_id='person-1',revision=1,preferences=[dict(key='language',value='English',status='APPROVED',provenance=p)],sensitivity='PRIVATE',provenance=p,deleted=False,corrects_revision=None),
'PersonalAgent':dict(agent_id='agent-1',person_id='person-1',profile_revision=1,display_name='UnoOne',persona_revision=1,conversation_refs=['thread-1'],capability_preferences=['mail.read']),
'TaskSpec':dict(task_id='task-1',agent_id='agent-1',person_id='person-1',idempotency_key='request-1',goal='Summarize permitted locally cached mail',origin_replica_id='phone-1',target_replica_id='phone-1',host_requirements=['ANDROID'],scopes=s,budget=b,created_at_ms=1000,deadline_ms=61000,expected_postcondition='Summary references permitted messages',user_visible_policy='Read and suggest only',sensitivity='PRIVATE'),
'TaskEvent':dict(event_id='event-1',operation_id='op-1',task_id='task-1',predecessor_event_id=None,origin_replica_id='phone-1',assigned_replica_id='phone-1',step=0,deadline_ms=61000,transition='PLANNED',evidence_ref=None,external_object_id=None),
'AgentSpec':dict(child_id='child-1',parent_task_id='task-1',purpose='Summarize permitted mail',template_version=1,model_selection='LOCAL_QUALIFIED_ONLY',scopes=s,budget=b,depth=1,expires_at_ms=61000,stop_generation=0,verifier_id='native-summary-v1'),
'TaskReceipt':dict(operation_id='op-1',task_id='task-1',attempt_id='attempt-1',actor_id='agent-1',replica_id='phone-1',before_evidence_refs=[],after_evidence_refs=['evidence-1'],dispatch_intent='OPEN_COMPOSER',observed_effect='Composer opened only',outcome='ACTION_VERIFIED',reason='No send observed',source='NATIVE',timestamp_ms=2000,external_object_id=None),
'Handoff':dict(handoff_id='handoff-1',task_id='task-1',from_replica_id='phone-1',target_replica_id='power-1',encrypted_goal='ciphertext-placeholder',receipt_operation_ids=['op-1'],host_requirements=['DESKTOP'],approval='PENDING',expires_at_ms=61000),
'ReplicaChange':dict(person_id='person-1',replica_id='phone-1',sequence=1,operation_id='change-1',record_id='task-1',record_kind='TASK_SPEC',predecessor_operation_id=None,record_revision=1,change='UPSERT',ciphertext='ciphertext-placeholder',content_hash='a'*64,provenance=p),
'MailAccount':dict(account_id='mail-1',person_id='person-1',provider='LOCAL',display_name='Synthetic mailbox',address='person@example.invalid'),
'MessageRef':dict(account_id='mail-1',message_id='message-1',thread_id='mail-thread-1',folder_id='folder-1'),
'Draft':dict(draft_id='draft-1',task_id='task-1',account_id='mail-1',reply_to=None,recipients=['person@example.invalid'],subject='Proposed meeting',body='Draft only',attachment_refs=[],revision=1,approval='PENDING'),
'CalendarRef':calendar,
'EventRef':dict(calendar=calendar,event_id='event-provider-1',start_ms=100000,end_ms=200000,time_zone='Asia/Kolkata',attendees=['person@example.invalid']),
'CapabilityGrant':dict(grant_id='grant-1',person_id='person-1',replica_id='phone-1',scopes=s,budget=b,issued_at_ms=1000,expires_at_ms=61000,stop_generation=0,revoked=False),
}
for r,payload in examples.items():
    (PKG/'fixtures'/(snake(r)+'.json')).write_text(json.dumps({'schema':'inbharat.pai.personal-agent','version':{'major':1,'minor':0},'kind':upper(r),'payload':payload},indent=2)+'\n')
# Kotlin dispatch is explicit: no polymorphic class name is accepted from input.
codec=[]
for r in RECORDS: codec.append(f'            RecordKind.{upper(r)} -> json.decodeFromJsonElement<{r}>(wire.payload)')
(KOTLIN/'PersonalRecordDispatch.kt').write_text('''// Generated; see tools/generate.py.
package com.unoone.agent.core.personal
import kotlinx.serialization.json.*
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
internal fun decodeRecord(wire: WireEnvelope): PersonalRecord {
    val json = PersonalCodec.json
    return when (wire.kind) {
'''+ '\n'.join(codec)+'''
    }
}
internal fun encodeRecord(record: PersonalRecord): Pair<RecordKind, JsonObject> {
    val json = PersonalCodec.json
    return when (record) {
'''+ '\n'.join(f'        is {r} -> RecordKind.{upper(r)} to json.encodeToJsonElement(record).jsonObject' for r in RECORDS)+'''
    }
}
''')
print('Generated',len(RECORDS),'DTO records, schema and golden fixtures')
