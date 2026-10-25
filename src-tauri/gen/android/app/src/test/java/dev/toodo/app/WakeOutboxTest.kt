package dev.toodo.app

import org.junit.Assert.*
import org.junit.Test

class WakeOutboxTest {
    private class Disk : WakeOutbox.Store {
        var value: WakePending? = null
        var writable = true
        override fun read() = value
        override fun write(message: WakePending?): Boolean {
            if (!writable) return false
            value = message
            return true
        }
    }

    @Test fun restoresTheSameIdentityTextAndScopeAfterRestart() {
        val disk = Disk()
        val original = WakeOutbox(disk).stage("message", "opaque-server-a")!!
        assertEquals(original, WakeOutbox(disk).pending)
    }

    @Test fun refusesToOverwritePendingMessageFromAnotherServer() {
        val disk = Disk()
        val outbox = WakeOutbox(disk)
        val original = outbox.stage("first", "a")
        assertNull(outbox.stage("second", "b"))
        assertEquals(original, WakeOutbox(disk).pending)
    }

    @Test fun staleAcknowledgementCannotDeletePendingMessage() {
        val disk = Disk()
        val outbox = WakeOutbox(disk)
        val original = outbox.stage("first", "a")
        assertTrue(outbox.acknowledge("stale-id"))
        assertEquals(original, WakeOutbox(disk).pending)
    }

    @Test fun matchingAcknowledgementDeletesDurably() {
        val disk = Disk()
        val outbox = WakeOutbox(disk)
        val message = outbox.stage("first", "a")!!
        assertTrue(outbox.acknowledge(message.id))
        assertNull(WakeOutbox(disk).pending)
    }

    @Test fun failedPersistenceDoesNotExposePublishableMessage() {
        val disk = Disk().apply { writable = false }
        val outbox = WakeOutbox(disk)
        assertNull(outbox.stage("first", "a"))
        assertNull(outbox.pending)
    }

    @Test fun failedAcknowledgementKeepsOriginalIdentityForRetry() {
        val disk = Disk()
        val outbox = WakeOutbox(disk)
        val message = outbox.stage("first", "a")!!
        disk.writable = false
        assertFalse(outbox.acknowledge(message.id))
        assertEquals(message, outbox.pending)
    }
}
