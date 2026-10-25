package dev.toodo.app

import android.Manifest
import android.app.Activity
import android.app.KeyguardManager
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.Build
import java.util.concurrent.Executors
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** One serial worker owns both ASR and AudioRecord, including their destruction. */
internal class WakeAudio(private val activity: Activity) {
    private val executor = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val lock = Any()
    private var recorder: AudioRecord? = null
    private var cancellation = AtomicBoolean(true)

    fun safe(): Boolean =
        activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED &&
            activity.getSystemService(PowerManager::class.java).isInteractive &&
            !activity.getSystemService(KeyguardManager::class.java).isDeviceLocked &&
            !activity.getSystemService(KeyguardManager::class.java).isKeyguardLocked &&
            activity.getSystemService(AudioManager::class.java).mode == AudioManager.MODE_NORMAL

    fun start(callbacks: Callbacks) {
        stop()
        val cancelled = AtomicBoolean(false)
        cancellation = cancelled
        executor.execute {
            try {
                val directory = WakeModel.prepare(activity.noBackupFilesDir) { !cancelled.get() && safe() }
                if (cancelled.get()) return@execute
                if (!safe()) {
                    main.post { callbacks.interrupted() }
                    return@execute
                }
                WakeRecognizer(directory.absolutePath).use { recognizer ->
                    val focus = FutureTask { !cancelled.get() && callbacks.ready() }
                    main.post(focus)
                    if (!focus.get(5, TimeUnit.SECONDS)) return@execute
                    synchronized(lock) {
                        if (cancelled.get()) return@execute
                        if (!safe()) {
                            main.post { callbacks.interrupted() }
                            return@execute
                        }
                        val minimum = AudioRecord.getMinBufferSize(16000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
                        check(minimum > 0) { "Microphone does not support 16 kHz mono PCM" }
                        recorder = AudioRecord(
                            MediaRecorder.AudioSource.VOICE_RECOGNITION, 16000,
                            AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
                            maxOf(minimum, 16000 * 2 * 2),
                        )
                        check(recorder?.state == AudioRecord.STATE_INITIALIZED) { "Microphone initialization failed" }
                        recorder!!.startRecording()
                        check(recorder?.recordingState == AudioRecord.RECORDSTATE_RECORDING) { "Microphone did not start" }
                    }
                    post(cancelled) { callbacks.state("listening") }
                    val pcm = ShortArray(1600)
                    while (!cancelled.get()) {
                        if (!safe()) {
                            synchronized(lock) { recorder?.stop() }
                            main.post { callbacks.interrupted() }
                            break
                        }
                        val count = recorder!!.read(pcm, 0, pcm.size, AudioRecord.READ_BLOCKING)
                        if (cancelled.get()) break
                        check(count > 0) { "Microphone read failed ($count)" }
                        if (Build.VERSION.SDK_INT >= 29) {
                            check(recorder?.activeRecordingConfiguration?.isClientSilenced != true) {
                                "Microphone was silenced by Android"
                            }
                        }
                        val wasCapturing = recognizer.capturing
                        val message = recognizer.feed(FloatArray(count) { pcm[it] / 32768.0f })
                        if (!wasCapturing && recognizer.capturing) post(cancelled) { callbacks.state("capturing") }
                        if (message != null) {
                            if (message.isNotEmpty()) {
                                // Stop the only microphone before publishing a completed sentence.
                                synchronized(lock) { recorder?.stop() }
                                post(cancelled) { callbacks.message(message) }
                                break
                            }
                            post(cancelled) { callbacks.state("listening") }
                        }
                    }
                }
            } catch (error: Exception) {
                post(cancelled) { callbacks.error(error.message ?: "Wake recognition failed") }
            } catch (error: LinkageError) {
                post(cancelled) { callbacks.error(error.message ?: "Wake runtime is unavailable") }
            } finally {
                synchronized(lock) {
                    recorder?.let {
                        if (it.recordingState == AudioRecord.RECORDSTATE_RECORDING) it.stop()
                        it.release()
                    }
                    recorder = null
                }
            }
        }
    }

    private fun post(cancelled: AtomicBoolean, action: () -> Unit) {
        main.post { if (!cancelled.get()) action() }
    }

    /** Called on the UI thread at every foreground/privacy boundary, never queued behind ASR. */
    fun stop() {
        cancellation.set(true)
        synchronized(lock) {
            recorder?.let {
                if (it.recordingState == AudioRecord.RECORDSTATE_RECORDING) it.stop()
            }
        }
    }

    fun close() {
        stop()
        executor.shutdown()
    }

    interface Callbacks {
        fun ready(): Boolean
        fun state(state: String)
        fun message(text: String)
        fun error(error: String)
        fun interrupted()
    }
}
