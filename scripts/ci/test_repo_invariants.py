"""Mutation tests exercise the real catalogue validator without editing repository assets."""
import copy
import unittest
import tempfile
from pathlib import Path
from unittest.mock import patch
import check_repo_invariants as gate

class ManifestContractTest(unittest.TestCase):
    def setUp(self):
        self.manifest = gate.load_json('android-app/UnoOneAgent/modelmanager/src/main/assets/models_manifest.json')
    def check(self, value):
        errors = []
        with patch.object(gate, 'load_json', return_value=value):
            gate.validate_model_manifest(errors)
        return errors
    def test_exact_four_profiles(self):
        self.assertEqual([], self.check(self.manifest))
        for mutate in [lambda m: m['models'].pop(0), lambda m: m['models'].append(copy.deepcopy(m['models'][0]))]:
            bad = copy.deepcopy(self.manifest); mutate(bad)
            self.assertTrue(self.check(bad))
    def test_both_pins_and_transport_are_enforced(self):
        for index in (0, 1):
            for key, value in [('sha256','0'*64), ('sizeBytes',1), ('url','https://attacker.invalid/resolve/main/model'), ('name','bad-web.litertlm'), ('archive',True), ('asset','override')]:
                bad=copy.deepcopy(self.manifest); bad['models'][index]['files'][0][key]=value
                self.assertTrue(self.check(bad), (index,key))
    def test_schema_ram_and_path_guardrails(self):
        for field, value in [('manifestVersion',3), ('manifestVersion',4)]:
            bad=copy.deepcopy(self.manifest);bad[field]=value;self.assertTrue(self.check(bad))
        for field,value in [('folder','../escape'),('minRamMb',1),('id','unknown')]:
            bad=copy.deepcopy(self.manifest);bad['models'][0][field]=value;self.assertTrue(self.check(bad))
    def test_qwen_every_artifact_is_exact_and_required(self):
        qi = next(i for i,m in enumerate(self.manifest['models']) if m['id'] == gate.QWEN_ID)
        for index in range(9):
            for key,value in [('sha256','0'*64),('sizeBytes',1),('url','https://attacker.invalid/remote'),('name','missing'),('archive',True),('asset','override')]:
                bad=copy.deepcopy(self.manifest);bad['models'][qi]['files'][index][key]=value
                self.assertTrue(self.check(bad),(index,key))
            bad=copy.deepcopy(self.manifest);bad['models'][qi]['files'].pop(index)
            self.assertTrue(self.check(bad),index)
        for key,value in [('backend','gpu'),('version','main'),('folder','brain/other'),('minRamMb',1)]:
            bad=copy.deepcopy(self.manifest);bad['models'][qi][key]=value
            self.assertTrue(self.check(bad),key)
    def test_owl_exact_pair_and_unknown_profile(self):
        oi = next(i for i,m in enumerate(self.manifest['models']) if m['id'] == gate.OWL_ID)
        for index in range(2):
            for key,value in [('sha256','0'*64),('sizeBytes',1),('name','wrong.gguf'),('archive',True),('asset','override'),('url', self.manifest['models'][oi]['files'][index]['url'].replace(gate.OWL_REVISION, 'main'))]:
                bad=copy.deepcopy(self.manifest);bad['models'][oi]['files'][index][key]=value
                self.assertTrue(self.check(bad),(index,key))
            bad=copy.deepcopy(self.manifest);bad['models'][oi]['files'].pop(index)
            self.assertTrue(self.check(bad),index)
        bad=copy.deepcopy(self.manifest);bad['models'][oi]['files'].append(copy.deepcopy(bad['models'][oi]['files'][0]))
        self.assertTrue(self.check(bad))
        for key,value in [('backend','gpu'),('version','main'),('folder','brain/other'),('minRamMb',1),('id','unknown-llm'),('type','asr')]:
            bad=copy.deepcopy(self.manifest);bad['models'][oi][key]=value
            self.assertTrue(self.check(bad),key)
        bad=copy.deepcopy(self.manifest);unknown=copy.deepcopy(bad['models'][oi]);unknown['id']='unknown';unknown['folder']='brain/unknown';bad['models'].append(unknown)
        self.assertTrue(self.check(bad))

    def test_v3_schema_corpus_and_negative_cases(self):
        errors=[];gate.validate_v3_corpus(errors);self.assertEqual([],errors)

class OwlSourceContractTest(unittest.TestCase):
    def test_current_contract(self):
        errors=[];gate.validate_owl_source_contract(errors);self.assertEqual([],errors)

    def test_runtime_provenance_and_verification_mutations(self):
        original=Path.read_text
        mutations = [
            ('BrainModel.kt','BrainRuntime.LLAMA_CPP','BrainRuntime.MNN'),
            ('BrainModel.kt','supportsBrowserProtocol = false','supportsBrowserProtocol = true'),
            ('BrainModel.kt','isDeviceVerified = false','isDeviceVerified = true'),
            ('BrainModel.kt','experimentalLabel = "EXPERIMENTAL','experimentalLabel = "VERIFIED'),
            ('BrainModel.kt','GuiOwlArtifact.PROVENANCE_DISCLOSURE','"cleared"'),
            ('GuiOwlArtifact.kt',gate.OWL_REVISION,'main'),
            ('GuiOwlArtifact.kt',gate.OWL_DISCLOSURE,'Commercially cleared'),
            ('GuiOwlArtifact.kt','2497282208L','1L'),
            ('GuiOwlArtifact.kt','453974336L','1L'),
        ]
        for filename,old,new in mutations:
            with self.subTest(filename=filename,old=old):
                def changed(path,*args,**kwargs):
                    text=original(path,*args,**kwargs)
                    return text.replace(old,new) if path.name == filename else text
                errors=[]
                with patch.object(Path,'read_text',changed):gate.validate_owl_source_contract(errors)
                self.assertTrue(errors)

class SourceScanTest(unittest.TestCase):
    def test_generated_metadata_skipped_but_native_app_sources_scanned(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);active=root/'android-app/UnoOneAgent'
            source=active/'localbrain/src/main/java/App.kt'
            generated=active/'localbrain/.cxx/Debug/targets.json'
            for path in (source,generated):
                path.parent.mkdir(parents=True,exist_ok=True)
                path.write_text('gemma3n')
            with patch.object(gate,'ROOT',root),patch.object(gate,'ACTIVE_ROOTS',(active,)):
                self.assertEqual([source],list(gate.iter_active_files()))
                errors=[];gate.scan_prohibited_text(errors)
                self.assertEqual(1,len(errors));self.assertIn('src/main/java/App.kt',errors[0])
                source.write_text('safe app source')
                errors=[];gate.scan_prohibited_text(errors);self.assertEqual([],errors)

class BrowserBoundaryTest(unittest.TestCase):
    def test_current_boundary(self):
        errors=[]; gate.validate_secure_browser(errors); self.assertEqual([],errors)

    def test_rejects_bridge_reintroduction_and_dynamic_code(self):
        from pathlib import Path
        original = Path.read_text
        for filename, injected in [('SecureWebViewController.kt', '\naddJavascriptInterface('), ('dom-adapter.js', '\nMODEL_INVOKE'), ('dom-adapter.js', '\neval(')]:
            def changed(path, *args, **kwargs):
                text=original(path,*args,**kwargs)
                return text + injected if path.name == filename else text
            errors=[]
            with patch.object(Path,'read_text',changed): gate.validate_secure_browser(errors)
            self.assertTrue(errors,(filename,injected))


class IntegratedRetentionContractTest(unittest.TestCase):
    android = Path(__file__).resolve().parents[2] / 'android-app/UnoOneAgent'

    def test_synced_eviction_executes_real_dao_sql_without_losing_dirty_or_local_rows(self):
        import re, sqlite3
        for dao, table, kind in [('NoteDao','notes','NOTE'), ('MemoryDao','memories','MEMORY'), ('ConversationTurnDao','conversation_turns','TRANSCRIPT')]:
            text = (self.android / f'storage/src/main/java/com/unoone/agent/storage/dao/{dao}.kt').read_text()
            for method in ['deleteOlderThanSynced', 'deleteSynced']:
                sql = re.search(r'@Query\("([^"\n]+)"\)\s+suspend fun ' + method, text).group(1)
                with sqlite3.connect(':memory:') as db:
                    db.execute(f'CREATE TABLE {table}(id INTEGER PRIMARY KEY, createdAt INTEGER, updatedAt INTEGER, vaultRecordId TEXT)')
                    db.execute('CREATE TABLE pending_writes(recordKind TEXT, localId INTEGER)')
                    db.executemany(f'INSERT INTO {table} VALUES(?,?,?,?)', [(1,0,0,None),(2,0,0,'dirty'),(3,0,0,'synced'),(4,0,0,'dirty-fact')])
                    db.executemany('INSERT INTO pending_writes VALUES(?,?)', [(kind,2),(kind,4)])
                    db.execute(sql, {'cutoff': 1000})
                    self.assertEqual([1,2,4], [row[0] for row in db.execute(f'SELECT id FROM {table} ORDER BY id')], (dao,method))

    def test_schema6_and_non_destructive_recovery_wiring(self):
        root = self.android
        provider = (root/'app/src/main/java/com/unoone/agent/di/DatabaseProvider.kt').read_text()
        key = (root/'storage/src/main/java/com/unoone/agent/storage/cache/CacheKeyManager.kt').read_text()
        self.assertIn('DatabaseOpenPolicy.requireSafeOpen', provider)
        self.assertIn('SupportOpenHelperFactory', provider)
        self.assertIn('MIGRATION_5_6', provider)
        self.assertNotIn('deleteDatabase(', provider)
        self.assertNotIn('fallbackToDestructiveMigration', provider)
        self.assertNotIn('wrappedKeyFile.delete()', key)
        self.assertIn('throw DatabaseRecoveryRequired', key)
        schema = (root/'storage/src/main/java/com/unoone/agent/storage/db/UnoOneDatabase.kt').read_text()
        self.assertIn('version = 6', schema)
        for entity in ['PendingWriteEntity','PendingTombstoneEntity','ConversationTurnEntity']:
            self.assertIn(entity, schema)

    def test_vault_callbacks_and_legacy_aliases_remain_guarded(self):
        root = self.android
        app = (root/'app/src/main/java/com/unoone/agent/UnoOneApplication.kt').read_text()
        for required in ['VaultMirror(', 'VaultHydrator(', 'EnvLearningRecorder(', 'conversationDao = db.conversationTurnDao()', 'databaseRecoveryMessage']:
            self.assertIn(required, app)
        executor = (root/'app/src/main/java/com/unoone/agent/execution/ActionExecutor.kt').read_text()
        for required in ['ToolCallValidator.rejection(normalized)', 'TaskToolAuthorization.handle(normalized)', 'execution.beforeEffect(', 'vaultMirror?.onNoteCreated(rowId)', 'vaultMirror?.onRowDeleted']:
            self.assertIn(required, executor)
        self.assertNotIn('clickNodeById(', executor)
        self.assertNotIn('typeIntoNodeById(', executor)
        mirror = (root/'app/src/main/java/com/unoone/agent/vaultbridge/VaultMirror.kt').read_text()
        for method in ['onMemoryUpserted','onSkillUpserted','onEnvFactRecorded']:
            body = mirror.split('suspend fun '+method,1)[1].split('} catch (e:',1)[0]
            self.assertLess(body.index('recordIdFor('), body.index('writerProvider()'))

    def test_action_verified_is_guarded_against_stale_summary(self):
        source = (self.android/'app/src/main/java/com/unoone/agent/task/TaskJournalStore.kt').read_text()
        guard = source.split('private fun append(',1)[1].split('val sequence =',1)[0]
        self.assertIn('"ACTION_VERIFIED"', guard)
        self.assertIn('return@synchronized', guard)

if __name__ == '__main__': unittest.main()
