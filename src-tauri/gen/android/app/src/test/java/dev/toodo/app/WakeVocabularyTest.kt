package dev.toodo.app

import org.junit.Assert.assertEquals
import org.junit.Test

class WakeVocabularyTest {
    @Test fun exportsPieceAndFloatScoreWhileSkippingOtherModelFields() {
        // ModelProto.pieces = {piece:"abc", score:-1.5, type:1}; trainer_spec is ignored.
        val model = byteArrayOf(10, 12, 10, 3, 97, 98, 99, 21, 0, 0, -64, -65, 24, 1, 18, 0)
        assertEquals("abc\t-1.5\n", WakeVocabulary.export(model))
    }

    @Test(expected = IllegalStateException::class)
    fun rejectsTruncatedPieceRatherThanGeneratingPartialVocabulary() {
        WakeVocabulary.export(byteArrayOf(10, 10, 10, 3, 97))
    }
}
