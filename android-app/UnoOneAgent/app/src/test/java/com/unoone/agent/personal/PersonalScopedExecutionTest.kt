package com.unoone.agent.personal

import com.unoone.agent.core.task.*
import com.unoone.agent.vault.*
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files
import java.io.File

class PersonalScopedExecutionTest {
    private fun accepted(): Pair<PersonalLedger, String> {
        var l = PersonalLedger.fresh(PersonalLedger.id()); val tid = PersonalLedger.id()
        for (action in listOf(PersonalAction.CREATE, PersonalAction.ACCEPT)) l = l.apply(PersonalRequest(PersonalLedger.id(),l.view().revision,action,tid,"test goal","",null,l.replica_id),System.currentTimeMillis())
        return l to tid
    }
    @Test fun actualEncryptedSelectedReadAndCoordinatorTestModelWritesLedger() = runBlocking {
        val dir=Files.createTempDirectory("scoped-personal-test").toFile()
        val scope=CoroutineScope(SupervisorJob()+Dispatchers.Default)
        try {
            val io=PrivateFileVaultIO(File(dir,"vault"));val repo=MobileVaultRepository(io);val session=repo.create("scoped-test-password".toByteArray())
            val id=PersonalLedger.id();val date=java.time.Instant.now().toString()
            val note=VaultRecordFactory.forNote(id,PersonalLedger.id(),"test","Selected note","source-marker","",date,date)
            repo.writeRecord(session,note.fields,note.content)
            val reads=mutableListOf<String>()
            val reader=object:VaultRecordReader {
                override fun listRecordMetadata()=repo.listRecordMetadata(session)
                override fun readRecord(recordId:String)=repo.readRecord(session,recordId).also { reads.add(recordId) }
            }
            var (ledger,tid)=accepted();val view=ledger.view()
            val permit=PersonalDraftPermit.approve(view,tid,view.revision,System.currentTimeMillis(),0,PersonalSource(listOf(id),"source-marker"),true)
            val source=PersonalSource.read(reader,permit,System.currentTimeMillis(),0)
            assertEquals(listOf(id),reads)
            val prompts=java.util.Collections.synchronizedList(mutableListOf<String>())
            // Explicit TEST MODEL callback. Production supplies the real model lease/chatForTask.
            val family=PersonalTaskFamily { _,prompt -> prompts.add(prompt); "TEST MODEL response only" }
            val coordinator=TaskCoordinator(family.registrations(),scope)
            val admitted=family.submit(coordinator,permit,source,"Independent draft",{}) as Admission.Accepted
            assertEquals(TaskOutcome.RESPONDED,withTimeout(3000){coordinator.await(admitted.taskId)}.outcome)
            val output=family.output(admitted.taskId)
            assertEquals(2,prompts.size);assertEquals(1,prompts.count { it.contains("source-marker") })
            ledger=ledger.recordDraftAttempt(view.revision,PersonalDraftPhase.STARTED,"",permit,System.currentTimeMillis(),0)
            ledger=ledger.recordDraftAttempt(view.revision+1,PersonalDraftPhase.RESPONDED,output,permit,System.currentTimeMillis(),0)
            val writer=object:VaultRecordWriter {
                override fun writeRecord(fields:Map<String,Any?>,content:ByteArray)=repo.writeRecord(session,fields,content)
                override fun tombstone(vaultRecordId:String,deletedAtIso:String)=repo.tombstoneRecord(session,vaultRecordId,deletedAtIso)
            }
            val store=PersonalStore(session.vaultId,reader,writer,{io.exists("VAULT/records/${PersonalLedger.RECORD_ID}.enc.json")})
            // Bind the fixture ledger to this actual vault, like production store load.
            val persisted=ledger.copy(local_vault_id=session.vaultId);store.save(persisted)
            assertEquals("AWAITING_VERIFICATION",store.load().view().tasks.single().status)
            assertFalse(io.read("VAULT/records/${PersonalLedger.RECORD_ID}.enc.json").toString(Charsets.UTF_8).contains("TEST MODEL"))
            family.discard(admitted.taskId);coordinator.close();session.close()
        } finally {scope.cancel();dir.deleteRecursively()}
    }
    @Test fun stopParentRejectsLateChildWhileUnrelatedTaskLives(): Unit = runBlocking {
        val scope=CoroutineScope(SupervisorJob()+Dispatchers.Default)
        val entered=CompletableDeferred<Unit>();val release=CompletableDeferred<Unit>()
        val (ledger,tid)=accepted();val v=ledger.view()
        val permit=PersonalDraftPermit.approve(v,tid,v.revision,System.currentTimeMillis(),0,children=true)
        val family=PersonalTaskFamily { _,_ -> entered.complete(Unit);withContext(NonCancellable){release.await()};"LATE TEST MODEL output" }
        val unrelated=WorkerKind("unrelated-test-only")
        val coordinator=TaskCoordinator(family.registrations()+WorkerRegistration(unrelated,WorkerLane.INTERACTIVE,NativeTaskWorker{WorkerResult.Finished(TaskResult(TaskOutcome.RESPONDED))}),scope)
        try {
            val parent=family.submit(coordinator,permit,"summary","draft",{}) as Admission.Accepted
            withTimeout(3000){entered.await()}
            val other=coordinator.submit(TaskRequest(RequestId(PersonalLedger.id()),unrelated,"",TaskScope(),coordinator.captureGeneration())) as Admission.Accepted
            coordinator.cancel(parent.taskId);release.complete(Unit)
            assertEquals(TaskOutcome.CANCELLED,withTimeout(3000){coordinator.await(parent.taskId)}.outcome)
            assertEquals(TaskOutcome.RESPONDED,withTimeout(3000){coordinator.await(other.taskId)}.outcome)
            assertThrows(IllegalStateException::class.java){family.output(parent.taskId)}
        } finally {coordinator.close();scope.cancel()}
    }
}
