package dev.toodo.app

import java.util.UUID

internal data class WakePending(val id: String, val text: String, val scope: String)

/** A successful durable write is the only way a message becomes publishable. */
internal class WakeOutbox(private val store: Store) {
    var pending: WakePending? = store.read()
        private set

    fun stage(text: String, scope: String): WakePending? {
        if (pending != null) return null
        val message = WakePending(UUID.randomUUID().toString(), text, scope)
        if (!store.write(message)) return null
        pending = message
        return message
    }

    fun acknowledge(id: String): Boolean {
        if (pending?.id != id) return true
        if (!store.write(null)) return false
        pending = null
        return true
    }

    interface Store {
        fun read(): WakePending?
        fun write(message: WakePending?): Boolean
    }
}
