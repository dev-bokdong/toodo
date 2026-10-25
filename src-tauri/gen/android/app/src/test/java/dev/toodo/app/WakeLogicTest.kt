package dev.toodo.app

import org.junit.Assert.*
import org.junit.Test

class WakeLogicTest {
    private val wake = "\ub3c4\ub9ac\uc57c"
    private val sentence = "\uc624\ub298\ud560\uc77c\uc54c\ub824\uc918"

    @Test fun preservesSentenceWhenWakeEndsInsideOneBpeToken() {
        val tokens = arrayOf("\u2581$wake$sentence")
        val match = WakeParser.find(tokens, floatArrayOf(0f))!!
        assertEquals(sentence, WakeParser.message(tokens, match))
    }

    @Test fun matchesLiaisonAndTwoEditPronunciations() {
        for (text in listOf("\ub3cc\uc774\uc57c", "\ub458\uc774\uc57c", "\ub4e4\uc774\uc57c", wake)) {
            assertNotNull(text, WakeParser.find(arrayOf(text), floatArrayOf(0f)))
        }
    }

    @Test fun rejectsMiddleSyllableWithoutRi() {
        assertNull(WakeParser.find(arrayOf("\ub3c4\uc2dc\uc758"), floatArrayOf(0f)))
    }

    @Test fun rejectsWakeInsidePreviousSpokenToken() {
        assertNull(WakeParser.find(arrayOf("\ud560$wake"), floatArrayOf(0f)))
    }

    @Test fun requiresHalfSecondFromPreviousSpokenToken() {
        assertNull(WakeParser.find(arrayOf("\ud560", wake), floatArrayOf(0f, 0.49f)))
        assertNotNull(WakeParser.find(arrayOf("\ud560", wake), floatArrayOf(0f, 0.5f)))
    }

    @Test fun endsTwoSecondsAfterMessageStopsChanging() {
        val tokens = arrayOf("$wake$sentence")
        val times = floatArrayOf(0f)
        val capture = WakeCapture(1.0, WakeParser.find(tokens, times)!!)
        assertFalse(capture.update(tokens, times, 1.0))
        assertFalse(capture.update(tokens, times, 2.99))
        assertTrue(capture.update(tokens, times, 3.0))
        assertEquals(sentence, capture.text)
    }

    @Test fun emptyWakeExpiresAtFiveSecondsNotEndpointSilence() {
        val tokens = arrayOf(wake)
        val times = floatArrayOf(0f)
        val capture = WakeCapture(1.0, WakeParser.find(tokens, times)!!)
        assertFalse(capture.update(tokens, times, 5.99))
        assertTrue(capture.update(tokens, times, 6.0))
        assertEquals("", capture.text)
    }

    @Test fun revisedBpeSegmentationDoesNotEatFirstMessageSyllable() {
        val tokens = arrayOf("\u2581$wake", sentence)
        val times = floatArrayOf(0f, 0.8f)
        val capture = WakeCapture(1.0, WakeParser.find(tokens, times)!!)
        capture.update(arrayOf("\u2581\ub3c4", "\ub9ac", "\uc57c${sentence}"), floatArrayOf(0f, 0.2f, 0.4f), 1.1)
        assertEquals(sentence, capture.text)
    }

    @Test fun everyPrivacyBoundaryPausesListening() {
        val ready = WakeConditions(enabled = true, resumed = true, interactive = true, unlocked = true,
            permitted = true, audioAvailable = true)
        assertEquals("listening", ready.state())
        for (blocked in listOf(ready.copy(suspended = true), ready.copy(resumed = false),
            ready.copy(interactive = false), ready.copy(unlocked = false),
            ready.copy(manual = true), ready.copy(audioAvailable = false))) {
            assertEquals("paused", blocked.state())
        }
        assertEquals("error", ready.copy(permitted = false).state())
        assertEquals("off", ready.copy(enabled = false).state())
    }

    @Test fun pendingOutboxBlocksListeningEvenAfterDisableOrScopeSwitch() {
        assertEquals("pending", WakeConditions(pending = true).state())
    }

    @Test fun suppressesReactivationForThreeHundredMillisecondsAfterSpeech() {
        val cooldown = WakeCooldown()
        cooldown.release(5000)
        assertFalse(cooldown.ready(5299))
        assertTrue(cooldown.ready(5300))
    }
}
