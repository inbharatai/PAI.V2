package com.unoone.agent.providers

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import kotlinx.serialization.*
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@Serializable internal class ProviderTokens(val access: String, val account: String, val scopes: Set<String>, val expiresMs: Long) {
    override fun toString() = "ProviderTokens(REDACTED)"
}
@Serializable internal data class AndroidOAuthConfig(val clientId: String) {
    fun validate() { require(clientId.matches(Regex("[A-Za-z0-9.-]+\\.apps\\.googleusercontent\\.com")) && clientId.length<=256) {"UNCONFIGURED: supply the registered Android OAuth client ID; SDK binds package and signer, not desktop redirect"} }
}
@Serializable internal class ProviderLocalState(val version: Int = 1, val vaultId: String, var config: AndroidOAuthConfig? = null, var tokens: ProviderTokens? = null, val entries: MutableList<PreparedEntry> = mutableListOf()) {
    override fun toString() = "ProviderLocalState(REDACTED)"
}
internal data class ProviderView(val status: String, val account: String?, val configured: Boolean, val prepared: List<PreparedEntry>)
/** Device-local AES-GCM with non-exportable AndroidKeyStore key. Never part of
 * vault mirror, Room, personal ledger, peer export, logs, model prompts or backups. */
internal class ProviderStore(context: Context, private val vaultId: String) {
    private val root=File(context.noBackupFilesDir,"provider-local").also { check(it.mkdirs() || it.isDirectory) }
    private val file=AtomicFile(File(root,"$vaultId.bin"))
    private val alias="unoone.provider.$vaultId"
    private val epoch=com.unoone.agent.vaultbridge.VaultConnection.sessionEpoch()
    private fun session() {check(com.unoone.agent.vaultbridge.VaultConnection.sessionEpoch()==epoch){"Vault session revoked; uncertain operation held"}}
    private fun key(create: Boolean): SecretKey {
        val ks=KeyStore.getInstance("AndroidKeyStore").apply{load(null)}
        (ks.getKey(alias,null) as? SecretKey)?.let{return it}
        check(create){"Protected key missing; retained state cannot be reset"}
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES,"AndroidKeyStore").apply {init(KeyGenParameterSpec.Builder(alias,KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT).setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setUnlockedDeviceRequired(true).build())}.generateKey()
    }
    fun load(): ProviderLocalState {
        require(java.util.UUID.fromString(vaultId).toString()==vaultId)
        check(!java.nio.file.Files.isSymbolicLink(root.toPath()) && !java.nio.file.Files.isSymbolicLink(file.baseFile.toPath()))
        if(!file.baseFile.exists() && !File(file.baseFile.path+".bak").exists()) return ProviderLocalState(vaultId=vaultId)
        require(file.baseFile.length()<=1048640)
        val encrypted=file.openRead().use {input-> val out=java.io.ByteArrayOutputStream();val buffer=ByteArray(8192);while(true){val n=input.read(buffer);if(n<0)break;require(out.size()+n<=1048640);out.write(buffer,0,n)};out.toByteArray()}
        require(encrypted.size>=29 && encrypted[0]==1.toByte())
        val cipher=Cipher.getInstance("AES/GCM/NoPadding");cipher.init(Cipher.DECRYPT_MODE,key(false),GCMParameterSpec(128,encrypted.copyOfRange(1,13)));cipher.updateAAD("provider-local-v1:$vaultId".toByteArray())
        val bytes=cipher.doFinal(encrypted,13,encrypted.size-13)
        return try {session();providerJson.decodeFromString<ProviderLocalState>(bytes.toString(Charsets.UTF_8)).also {require(it.version==1 && it.vaultId==vaultId && it.entries.size<=128)}}catch(_:Exception){throw IllegalStateException("Protected provider state invalid/session revoked; retained without reset")}finally{bytes.fill(0)}
    }
    fun save(state: ProviderLocalState) {
        session()
        require(state.vaultId==vaultId && state.entries.size<=128)
        val bytes=providerJson.encodeToString(state).toByteArray();require(bytes.size<=1048576)
        val cipher=Cipher.getInstance("AES/GCM/NoPadding");cipher.init(Cipher.ENCRYPT_MODE,key(true));cipher.updateAAD("provider-local-v1:$vaultId".toByteArray())
        val encrypted=try {cipher.doFinal(bytes)}finally{bytes.fill(0)}
        val stream=file.startWrite();try {stream.write(byteArrayOf(1));stream.write(cipher.iv);stream.write(encrypted);file.finishWrite(stream)}catch(e:Exception){file.failWrite(stream);throw IllegalStateException("Protected provider write failed; uncertain mutation must reconcile")}
    }
}
