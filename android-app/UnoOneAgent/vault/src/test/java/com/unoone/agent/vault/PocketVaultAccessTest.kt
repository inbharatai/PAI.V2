package com.unoone.agent.vault

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PocketVaultAccessTest {
    private val validManifest = """
        {
          "product_id": "com.inbharatai.unoone.pocket-ai",
          "schema_version": 2,
          "pai_version": "0.6.0-alpha",
          "vault": {
            "id_path": "VAULT/identity/vault.id",
            "expected_id": "prototype-1"
          }
        }
    """.trimIndent()

    @Test
    fun acceptsMatchingPocketIdentity() {
        val result = PocketVaultAccess.validateIdentity(
            validManifest,
            "0.6.0-alpha",
            "prototype-1"
        )
        assertTrue(result is PocketVaultResult.Valid)
        assertEquals("prototype-1", (result as PocketVaultResult.Valid).vaultId)
    }

    @Test
    fun rejectsOrdinaryOrSpoofedManifest() {
        val result = PocketVaultAccess.validateIdentity(
            validManifest.replace(PocketVaultAccess.PRODUCT_ID, "example.other"),
            "0.6.0-alpha",
            "prototype-1"
        )
        assertTrue(result is PocketVaultResult.Invalid)
    }

    @Test
    fun rejectsVaultIdentityMismatch() {
        val result = PocketVaultAccess.validateIdentity(
            validManifest,
            "0.6.0-alpha",
            "different"
        )
        assertTrue(result is PocketVaultResult.Invalid)
    }
}
