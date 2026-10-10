package com.unoone.agent.vault

import android.app.ActivityManager
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.Uri

/** Private initializer: obtains current OS memory information for each KDF.
 * No records, files, keys or passwords are exposed as provider operations.
 * Missing service/probe fails closed rather than guessing from Java maxMemory.
 */
class VaultMemoryProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        val app = context?.applicationContext ?: return false
        val manager = app.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return false
        NativeVaultKdf.installAndroidMemoryProbe {
            val info = ActivityManager.MemoryInfo()
            manager.getMemoryInfo(info)
            NativeVaultKdf.Memory(info.availMem, info.lowMemory, info.threshold)
        }
        return true
    }
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? = throw UnsupportedOperationException()
    override fun getType(uri: Uri): String? = throw UnsupportedOperationException()
    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = throw UnsupportedOperationException()
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int = throw UnsupportedOperationException()
}
