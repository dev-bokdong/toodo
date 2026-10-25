package dev.toodo.app

import android.Manifest
import android.app.Activity
import android.app.KeyguardManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.view.HapticFeedbackConstants
import app.tauri.plugin.JSObject
import org.json.JSONObject

/** Main-thread lifecycle and durable outbox; the worker cannot publish across a scope change. */
internal class WakeSession(
    private val activity: Activity,
    private val emit: (String, JSObject) -> Unit,
) {
    private val main = Handler(Looper.getMainLooper())
    private val audio = activity.getSystemService(AudioManager::class.java)
    private val worker = WakeAudio(activity)
    private var enabled = false
    private var suspended = false
    private var resumed = false
    private var manual = false
    private var scope = ""
    private var active = false
    private var generation = 0L
    private var focused = false
    private var focusLost = false
    private var screenAvailable = true
    private var failure: String? = null
    private val cooldown = WakeCooldown()
    private var state = "off"
    private var publishedError: String? = null
    private val outbox = WakeOutbox(WakePreferences(activity))
    private val focusListener = AudioManager.OnAudioFocusChangeListener { change ->
        focusLost = change != AudioManager.AUDIOFOCUS_GAIN
        reconcile()
    }
    private var focusRequest: AudioFocusRequest? = null
    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                Intent.ACTION_SCREEN_OFF -> {
                    screenAvailable = false
                    stop()
                }
                Intent.ACTION_SCREEN_ON, Intent.ACTION_USER_PRESENT -> screenAvailable = true
                AudioManager.ACTION_AUDIO_BECOMING_NOISY -> {
                    failure = "Audio route changed"
                    stop()
                }
            }
            reconcile()
        }
    }
    private val modeListener = if (Build.VERSION.SDK_INT >= 31) AudioManager.OnModeChangedListener { reconcile() } else null
    // Older Android versions have no mode-change listener. Recheck only while resumed;
    // the PCM loop independently checks before every read.
    private val recheck = object : Runnable {
        override fun run() {
            reconcile()
            if (resumed) main.postDelayed(this, 300)
        }
    }

    init {
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_OFF)
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_USER_PRESENT)
            addAction(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        }
        if (Build.VERSION.SDK_INT >= 33) activity.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        else activity.registerReceiver(receiver, filter)
        if (Build.VERSION.SDK_INT >= 31) modeListener?.let { audio.addOnModeChangedListener(activity.mainExecutor, it) }
        if (outbox.pending != null) state = "pending"
    }

    fun configure(enabled: Boolean, suspended: Boolean, scope: String) {
        failure = null
        if (this.suspended && !suspended) cooldown.release(SystemClock.elapsedRealtime())
        if (this.scope != scope || this.enabled != enabled || this.suspended != suspended) {
            stop()
            failure = null
        }
        this.enabled = enabled
        this.suspended = suspended
        this.scope = scope
        reconcile()
    }

    fun resume() {
        abandonFocus()
        resumed = true
        screenAvailable = activity.getSystemService(PowerManager::class.java).isInteractive
        failure = null
        main.removeCallbacks(recheck)
        recheck.run()
    }

    fun pause() {
        resumed = false
        main.removeCallbacks(recheck)
        stop()
        abandonFocus()
        reconcile()
    }

    fun manual(value: Boolean) {
        manual = value
        if (value) stop()
        else cooldown.release(SystemClock.elapsedRealtime())
        reconcile()
    }

    fun cancel() {
        stop()
        cooldown.release(SystemClock.elapsedRealtime())
        failure = null
        reconcile()
    }

    fun state(): JSObject = JSObject().put("state", state).put("error", failure ?: JSONObject.NULL)
    fun pending(): JSObject? = outbox.pending?.json()

    fun acknowledge(id: String): Boolean {
        if (!outbox.acknowledge(id)) return false
        failure = null
        reconcile()
        return true
    }

    private fun conditions() = WakeConditions(
        enabled = enabled,
        suspended = suspended || !cooldown.ready(SystemClock.elapsedRealtime()),
        resumed = resumed,
        interactive = screenAvailable && activity.getSystemService(PowerManager::class.java).isInteractive,
        unlocked = !activity.getSystemService(KeyguardManager::class.java).isDeviceLocked &&
            !activity.getSystemService(KeyguardManager::class.java).isKeyguardLocked,
        manual = manual,
        audioAvailable = !focusLost && audio.mode == AudioManager.MODE_NORMAL,
        permitted = activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED,
        pending = outbox.pending != null,
    )

    private fun publish(next: String) {
        if (next != state || failure != publishedError) {
            state = next
            publishedError = failure
            emit("wakeState", state())
        }
    }

    private fun reconcile() {
        if (failure == "Microphone permission is not granted" &&
            activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) failure = null
        val next = conditions().state()
        if (next != "listening" || failure != null) {
            stop()
            if (!resumed || !enabled || suspended || manual || outbox.pending != null) abandonFocus()
            if (next == "error") failure = "Microphone permission is not granted"
            publish(if (failure != null && next == "listening") "error" else next)
            return
        }
        if (active) return
        active = true
        val current = ++generation
        val capturedScope = scope
        publish("preparing") // No microphone or audio focus while downloading/loading.
        worker.start(object : WakeAudio.Callbacks {
            override fun ready(): Boolean {
                if (current != generation) return false
                if (conditions().state() != "listening") {
                    stop()
                    reconcile()
                    return false
                }
                if (!focused && !requestFocus()) {
                    focusLost = true
                    stop()
                    publish("paused")
                    return false
                }
                return true
            }
            override fun state(state: String) {
                if (current != generation) return
                if (state == "capturing") {
                    // A speaker tone would enter this same live stream or mask the first syllable.
                    activity.window.decorView.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                }
                publish(state)
            }
            override fun message(text: String) {
                if (current != generation || outbox.pending != null) return
                // Synchronous commit precedes the event and any possible JS/server send.
                val message = outbox.stage(text, capturedScope)
                if (message == null) {
                    error("Could not persist wake message")
                    return
                }
                reconcile()
                emit("wakeMessage", message.json())
            }
            override fun error(error: String) {
                if (current != generation) return
                failure = error
                stop()
                abandonFocus()
                publish("error")
            }
            override fun interrupted() {
                if (current != generation) return
                stop()
                reconcile()
            }
        })
    }

    private fun requestFocus(): Boolean {
        val result = if (Build.VERSION.SDK_INT >= 26) {
            val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE)
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANT)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                .setOnAudioFocusChangeListener(focusListener, main).build()
            focusRequest = request
            audio.requestAudioFocus(request)
        } else {
            @Suppress("DEPRECATION")
            audio.requestAudioFocus(focusListener, AudioManager.STREAM_MUSIC, AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
        }
        focused = result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        return focused
    }

    private fun abandonFocus() {
        if (focused) {
            if (Build.VERSION.SDK_INT >= 26) focusRequest?.let { audio.abandonAudioFocusRequest(it) }
            else {
                @Suppress("DEPRECATION")
                audio.abandonAudioFocus(focusListener)
            }
        }
        focused = false
        focusLost = false
    }

    private fun stop() {
        if (!active) return
        active = false
        generation++
        worker.stop()
    }

    fun close() {
        pause()
        activity.unregisterReceiver(receiver)
        if (Build.VERSION.SDK_INT >= 31) modeListener?.let { audio.removeOnModeChangedListener(it) }
        worker.close()
    }
}
