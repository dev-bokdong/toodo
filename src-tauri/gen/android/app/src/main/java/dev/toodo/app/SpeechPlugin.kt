package dev.toodo.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class StartArgs {
    var lang: String? = null
    var partialResults: Boolean = true
}

@InvokeArg
class ConfigureWakeArgs {
    var enabled: Boolean = false
    var suspended: Boolean = false
    var scope: String = ""
}

@InvokeArg
class AcknowledgeWakeArgs {
    lateinit var id: String
}

/**
 * Speech recognition by the phone for dictation in the lead chat (see `src/lib/native-speech.ts`):
 * Android's SpeechRecognizer, one utterance at a time. What is heard, a failure, and the end come
 * as the events "result", "error" and "end"; failures use the Web Speech API's error codes, so the
 * page handles both kinds of recognition alike. Leaving the app drops the utterance and frees the
 * microphone.
 */
@TauriPlugin(permissions = [Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone")])
class SpeechPlugin(private val activity: Activity) : Plugin(activity) {
    private var recognizer: SpeechRecognizer? = null
    private val wake by lazy { WakeSession(activity) { event, value -> trigger(event, value) } }

    override fun load(webView: WebView) {
        // A plugin registered after Activity.onResume still needs the initial foreground state.
        activity.runOnUiThread {
            val lifecycle = (activity as LifecycleOwner).lifecycle
            if (lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) wake.resume()
        }
    }

    @Command
    fun configureWake(invoke: Invoke) {
        val args = invoke.parseArgs(ConfigureWakeArgs::class.java)
        activity.runOnUiThread {
            wake.configure(args.enabled, args.suspended, args.scope)
            invoke.resolve()
        }
    }

    @Command
    fun wakeState(invoke: Invoke) {
        activity.runOnUiThread { invoke.resolve(wake.state()) }
    }

    @Command
    fun pendingWake(invoke: Invoke) {
        activity.runOnUiThread {
            val pending = wake.pending()
            if (pending == null) invoke.resolve() else invoke.resolve(pending)
        }
    }

    @Command
    fun cancelWake(invoke: Invoke) {
        activity.runOnUiThread {
            wake.cancel()
            invoke.resolve()
        }
    }

    @Command
    fun acknowledgeWake(invoke: Invoke) {
        val args = invoke.parseArgs(AcknowledgeWakeArgs::class.java)
        activity.runOnUiThread {
            if (wake.acknowledge(args.id)) invoke.resolve()
            else invoke.reject("Could not acknowledge wake message")
        }
    }

    @Command
    fun start(invoke: Invoke) {
        val args = invoke.parseArgs(StartArgs::class.java)
        if (getPermissionState("microphone") != PermissionState.GRANTED) {
            invoke.reject("not-allowed")
            return
        }
        if (!SpeechRecognizer.isRecognitionAvailable(activity)) {
            invoke.reject("service-not-allowed")
            return
        }
        activity.runOnUiThread {
            abort()
            wake.manual(true)
            try {
                val current = SpeechRecognizer.createSpeechRecognizer(activity)
                recognizer = current
                current.setRecognitionListener(listener(current))
                val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                    putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                    args.lang?.takeIf { it.isNotBlank() }?.let { putExtra(RecognizerIntent.EXTRA_LANGUAGE, it) }
                    putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, args.partialResults)
                    putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
                    putExtra(RecognizerIntent.EXTRA_SPEECH_INPUT_COMPLETE_SILENCE_LENGTH_MILLIS, 2000L)
                }
                current.startListening(intent)
                invoke.resolve()
            } catch (error: RuntimeException) {
                recognizer?.destroy()
                recognizer = null
                wake.manual(false)
                invoke.reject("audio-capture", error)
            }
        }
    }

    /** Stops listening; what was heard still comes as the final result. */
    @Command
    fun stop(invoke: Invoke) {
        activity.runOnUiThread {
            recognizer?.stopListening()
            invoke.resolve()
        }
    }

    @Command
    fun cancel(invoke: Invoke) {
        activity.runOnUiThread {
            abort()
            invoke.resolve()
        }
    }

    @Command
    fun openSettings(invoke: Invoke) {
        val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", activity.packageName, null))
        activity.startActivity(intent)
        invoke.resolve()
    }

    override fun onPause(activity: AppCompatActivity) {
        wake.pause()
        abort()
    }

    override fun onStop(activity: AppCompatActivity) {
        wake.pause()
    }

    override fun onResume(activity: AppCompatActivity) {
        wake.resume()
    }

    override fun onDestroy(activity: AppCompatActivity) {
        wake.close()
        abort()
    }

    private fun abort() {
        val current = recognizer ?: return
        current.cancel()
        trigger("error", JSObject().put("error", "aborted"))
        finish(current)
    }

    private fun finish(current: SpeechRecognizer) {
        if (current !== recognizer) return
        recognizer = null
        current.destroy()
        trigger("end", JSObject())
        wake.manual(false)
    }

    private fun listener(current: SpeechRecognizer) = object : RecognitionListener {
        override fun onPartialResults(results: Bundle) {
            if (current === recognizer) heard(results)?.let { result(it, false) }
        }

        override fun onResults(results: Bundle) {
            if (current !== recognizer) return
            result(heard(results) ?: "", true)
            finish(current)
        }

        override fun onError(error: Int) {
            if (current !== recognizer) return
            trigger("error", JSObject().put("error", code(error)))
            finish(current)
        }

        override fun onReadyForSpeech(params: Bundle?) {}
        override fun onBeginningOfSpeech() {}
        override fun onRmsChanged(rmsdB: Float) {}
        override fun onBufferReceived(buffer: ByteArray?) {}
        override fun onEndOfSpeech() {}
        override fun onEvent(eventType: Int, params: Bundle?) {}
    }

    private fun heard(results: Bundle): String? =
        results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()

    private fun result(text: String, final: Boolean) {
        trigger("result", JSObject().put("text", text).put("final", final))
    }

    private fun code(error: Int): String = when (error) {
        SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> "no-speech"
        SpeechRecognizer.ERROR_NETWORK, SpeechRecognizer.ERROR_NETWORK_TIMEOUT, SpeechRecognizer.ERROR_SERVER -> "network"
        SpeechRecognizer.ERROR_AUDIO -> "audio-capture"
        SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "not-allowed"
        SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED, SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE -> "language-not-supported"
        else -> "recognizer-error-$error"
    }
}
