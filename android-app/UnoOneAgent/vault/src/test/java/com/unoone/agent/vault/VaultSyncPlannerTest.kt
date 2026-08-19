package com.unoone.agent.vault

import com.unoone.agent.vault.VaultSyncPlanner.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class VaultSyncPlannerTest {

    @Test
    fun `writes come before tombstones and are ordered by localId`() {
        val ops = VaultSyncPlanner.plan(
            writes = listOf(
                VaultSyncPlanner.PendingWrite(30, Kind.NOTE),
                VaultSyncPlanner.PendingWrite(10, Kind.MEMORY),
                VaultSyncPlanner.PendingWrite(20, Kind.NOTE),
            ),
            tombstones = listOf(
                VaultSyncPlanner.PendingTombstone("rec-a", "2026-08-02T10:00:00+00:00"),
            ),
        )
        assertEquals(
            listOf(
                VaultSyncPlanner.Op.Write(10, Kind.MEMORY),
                VaultSyncPlanner.Op.Write(20, Kind.NOTE),
                VaultSyncPlanner.Op.Write(30, Kind.NOTE),
                VaultSyncPlanner.Op.Tombstone("rec-a", "2026-08-02T10:00:00+00:00"),
            ),
            ops,
        )
    }

    @Test
    fun `blank tombstone ids are dropped`() {
        val ops = VaultSyncPlanner.plan(
            writes = emptyList(),
            tombstones = listOf(
                VaultSyncPlanner.PendingTombstone("", "t"),
                VaultSyncPlanner.PendingTombstone("rec-b", "t"),
            ),
        )
        assertEquals(listOf(VaultSyncPlanner.Op.Tombstone("rec-b", "t")), ops)
    }

    @Test
    fun `duplicate tombstone ids collapse to one, last deletedAt wins`() {
        val ops = VaultSyncPlanner.plan(
            writes = emptyList(),
            tombstones = listOf(
                VaultSyncPlanner.PendingTombstone("rec-c", "first"),
                VaultSyncPlanner.PendingTombstone("rec-c", "second"),
            ),
        )
        assertEquals(listOf(VaultSyncPlanner.Op.Tombstone("rec-c", "second")), ops)
    }

    @Test
    fun `empty backlog yields no ops`() {
        assertTrue(VaultSyncPlanner.plan(emptyList(), emptyList()).isEmpty())
    }
}
