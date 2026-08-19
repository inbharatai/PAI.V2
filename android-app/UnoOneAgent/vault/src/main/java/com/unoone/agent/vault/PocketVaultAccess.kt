package com.unoone.agent.vault

import android.content.Context
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

object PocketVaultAccess {
    const val PRODUCT_ID = "com.inbharatai.unoone.pocket-ai"
    const val SCHEMA_VERSION = 2

    private val json = Json {
        ignoreUnknownKeys = true
        explicitNulls = false
    }

    fun validateTree(context: Context, treeUri: Uri): PocketVaultResult {
        val selected = DocumentFile.fromTreeUri(context, treeUri)
            ?: return PocketVaultResult.Invalid("The selected location cannot be opened.")
        val root = when {
            selected.findFile("manifest.json")?.isFile == true -> selected
            selected.findFile("UNOONE")?.isDirectory == true -> selected.findFile("UNOONE")!!
            else -> return PocketVaultResult.Invalid(
                "Select the Pocket AI root (UNOONE). manifest.json was not found."
            )
        }
        val manifestFile = root.findFile("manifest.json")
            ?: return PocketVaultResult.Invalid("manifest.json is missing.")
        val versionFile = root.findFile("VERSION")
            ?: return PocketVaultResult.Invalid("VERSION is missing.")
        val vaultIdFile = root.findFile("VAULT")
            ?.findFile("identity")
            ?.findFile("vault.id")
            ?: return PocketVaultResult.Invalid("VAULT/identity/vault.id is missing.")

        val manifestText = context.contentResolver.openInputStream(manifestFile.uri)
            ?.bufferedReader()
            ?.use { it.readText() }
            ?: return PocketVaultResult.Invalid("manifest.json cannot be read.")
        val version = context.contentResolver.openInputStream(versionFile.uri)
            ?.bufferedReader()
            ?.use { it.readText().trim() }
            ?: return PocketVaultResult.Invalid("VERSION cannot be read.")
        val vaultId = context.contentResolver.openInputStream(vaultIdFile.uri)
            ?.bufferedReader()
            ?.use { it.readText().trim() }
            ?: return PocketVaultResult.Invalid("vault.id cannot be read.")

        return validateIdentity(manifestText, version, vaultId, root.uri)
    }

    fun validateIdentity(
        manifestText: String,
        version: String,
        vaultId: String,
        rootUri: Uri? = null
    ): PocketVaultResult {
        val manifest = try {
            json.decodeFromString<PocketManifestIdentity>(manifestText)
        } catch (error: Exception) {
            return PocketVaultResult.Invalid(
                "manifest.json is not a supported Pocket AI schema: ${error.message}"
            )
        }
        if (manifest.productId != PRODUCT_ID) {
            return PocketVaultResult.Invalid("The selected drive is not a Pocket AI.")
        }
        if (manifest.schemaVersion != SCHEMA_VERSION) {
            return PocketVaultResult.Invalid(
                "Pocket AI schema ${manifest.schemaVersion} is unsupported; expected $SCHEMA_VERSION."
            )
        }
        if (version != manifest.paiVersion) {
            return PocketVaultResult.Invalid("VERSION does not match manifest.json.")
        }
        if (vaultId.isBlank()) {
            return PocketVaultResult.Invalid("vault.id is empty.")
        }
        if (manifest.vault.expectedId != null && manifest.vault.expectedId != vaultId) {
            return PocketVaultResult.Invalid("vault.id does not match manifest.json.")
        }
        return PocketVaultResult.Valid(rootUri, vaultId, manifest.paiVersion)
    }
}

@Serializable
private data class PocketManifestIdentity(
    @SerialName("product_id") val productId: String,
    @SerialName("schema_version") val schemaVersion: Int,
    @SerialName("pai_version") val paiVersion: String,
    val vault: PocketVaultIdentity
)

@Serializable
private data class PocketVaultIdentity(
    @SerialName("id_path") val idPath: String,
    @SerialName("expected_id") val expectedId: String? = null
)

sealed interface PocketVaultResult {
    data class Valid(
        val rootUri: Uri?,
        val vaultId: String,
        val paiVersion: String
    ) : PocketVaultResult

    data class Invalid(val reason: String) : PocketVaultResult
}
