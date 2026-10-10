package com.unoone.agent.storage.db

import android.content.Context
import androidx.room.*
import androidx.sqlite.db.SupportSQLiteDatabase
import androidx.sqlite.db.framework.FrameworkSQLiteOpenHelperFactory
import androidx.sqlite.db.SupportSQLiteOpenHelper
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.cache.LegacyRoomSchema
import com.unoone.agent.storage.entity.*
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

// Frozen v6 entity shapes: a generated Room v6 database validates the intermediate migration,
// then a second generated Room v7 validates the final migration. Not just executing source SQL.
@Entity(tableName = "notes", indices = [Index("title"),Index("tags"),Index("createdAt")])
data class SixNote(@PrimaryKey(autoGenerate=true) val id: Long = 0, val title: String, val content: String,
    val tags: String = "", val createdAt: Long = 1, val updatedAt: Long = 1, val reminderTime: Long? = null,
    val vaultRecordId: String? = null)
@Entity(tableName="pending_writes", indices=[Index(value=["recordKind","localId"],unique=true)])
data class SixWrite(@PrimaryKey(autoGenerate=true) val id: Long=0, val recordKind:String, val localId:Long,
    val recordId:String, val createdAt:Long=1)
@Entity(tableName="pending_tombstones")
data class SixDelete(@PrimaryKey(autoGenerate=true) val id:Long=0, val vaultRecordId:String,
    val recordKind:String, val deletedAtIso:String, val createdAt:Long=1)
@Dao interface SixDao {
    @Query("SELECT * FROM notes ORDER BY id") fun notes(): List<SixNote>
    @Insert fun note(n:SixNote):Long
    @Insert fun pending(p:SixWrite):Long
    @Insert fun tombstone(t:SixDelete):Long
}
@Database(entities=[SixNote::class,SixWrite::class,SixDelete::class,MemoryEntity::class,SkillEntity::class,
    ActionLogEntity::class,ModelMetadataEntity::class,ConversationTurnEntity::class],version=6,exportSchema=false)
abstract class SixDatabase:RoomDatabase() { abstract fun fixture():SixDao }

@RunWith(RobolectricTestRunner::class)
@Config(sdk=[34], manifest=Config.NONE)
class VaultOutboxMigrationTest {
    private val migrations = arrayOf(UnoOneDatabase.MIGRATION_1_2,UnoOneDatabase.MIGRATION_2_3,
        UnoOneDatabase.MIGRATION_3_4,UnoOneDatabase.MIGRATION_4_5,UnoOneDatabase.MIGRATION_5_6)

    @Test fun bothLegacyVersionsValidateAtSixThenSevenAndRetainValues() = runBlocking {
        val context = ApplicationProvider.getApplicationContext<Context>()
        for (version in listOf(1,2)) {
            val name = "legacy-$version-durable.db"
            context.deleteDatabase(name)
            val helper = FrameworkSQLiteOpenHelperFactory().create(SupportSQLiteOpenHelper.Configuration.builder(context)
                .name(name).callback(object:SupportSQLiteOpenHelper.Callback(version) {
                    override fun onCreate(db:SupportSQLiteDatabase) {
                        LegacyRoomSchema.tables.keys.forEach { db.execSQL(LegacyRoomSchema.create(it)) }
                        if(version==2) LegacyRoomSchema.indexes.forEach { db.execSQL(it.sql) }
                        db.execSQL("INSERT INTO notes VALUES (1,'fixture','exact retained value','fixture',11,12,NULL)")
                        db.execSQL("INSERT INTO memories VALUES (1,'fixture','retained preference','preference',11,12)")
                        db.execSQL("INSERT INTO skills VALUES (1,'fixture','fixture','[]',0,0,11,12)")
                    }
                    override fun onUpgrade(db:SupportSQLiteDatabase,oldVersion:Int,newVersion:Int) = error("unexpected")
                }).build())
            helper.writableDatabase; helper.close()
            val six = Room.databaseBuilder(context,SixDatabase::class.java,name).allowMainThreadQueries()
                .addMigrations(*migrations).build()
            assertEquals("exact retained value",six.fixture().notes().single().content)
            assertEquals(6,six.openHelper.writableDatabase.version)
            // Real prior pending identity must not be mistaken for first-install backlog.
            six.fixture().pending(SixWrite(recordKind="NOTE",localId=1,recordId="historical-pending"))
            six.fixture().tombstone(SixDelete(vaultRecordId="historical-deleted",recordKind="NOTE",deletedAtIso="2026-01-01T00:00:00Z"))
            six.close()
            val seven = Room.databaseBuilder(context,UnoOneDatabase::class.java,name).allowMainThreadQueries().build()
            assertEquals(7,seven.openHelper.writableDatabase.version)
            assertEquals("exact retained value",seven.noteDao().getById(1)!!.content)
            assertEquals("retained preference",seven.memoryDao().getByIdOnce(1)!!.value)
            assertFalse(seven.skillDao().getById(1)!!.enabled)
            val pending = seven.pendingWriteDao().get("NOTE",1)!!
            assertEquals("historical-pending",pending.recordId)
            assertEquals("HISTORICAL",pending.origin)
            assertTrue(seven.pendingWriteDao().hasHistoricalAuthority())
            assertEquals("historical-deleted",seven.pendingTombstoneDao().getAll().single().vaultRecordId)
            assertEquals("FRESH",seven.pendingWriteDao().get("MEMORY",1)!!.origin)
            seven.close()
            val reopened=Room.databaseBuilder(context,UnoOneDatabase::class.java,name).allowMainThreadQueries().build()
            assertEquals(pending,reopened.pendingWriteDao().get("NOTE",1))
            reopened.close(); context.deleteDatabase(name)
        }
    }
}
