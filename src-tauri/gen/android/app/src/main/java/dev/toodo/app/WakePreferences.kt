package dev.toodo.app

import android.content.Context
import app.tauri.plugin.JSObject

internal fun WakePending.json(): JSObject =
    JSObject().put("id", id).put("text", text).put("scope", scope)

/** commit(), not apply(): publication must follow durable storage. */
internal class WakePreferences(context: Context) : WakeOutbox.Store {
    private val preferences = context.getSharedPreferences("toodo-wake", Context.MODE_PRIVATE)

    override fun read(): WakePending? = preferences.getString("pending", null)?.let {
        val saved = JSObject(it)
        WakePending(saved.getString("id"), saved.getString("text"), saved.getString("scope"))
    }

    override fun write(message: WakePending?): Boolean {
        val edit = preferences.edit()
        if (message == null) edit.remove("pending") else edit.putString("pending", message.json().toString())
        return edit.commit()
    }
}
