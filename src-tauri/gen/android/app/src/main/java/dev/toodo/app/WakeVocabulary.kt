package dev.toodo.app

import java.nio.ByteBuffer
import java.nio.ByteOrder

/** SentencePiece ModelProto -> piece<TAB>score, without shipping a tokenizer runtime.
 * Only the repeated pieces (field 1) are needed; the checksum pins the input schema.
 */
internal object WakeVocabulary {
    private class Proto(private val data: ByteArray) {
        var offset = 0
        val ready get() = offset < data.size
        fun varint(): Int {
            var value = 0
            var shift = 0
            do {
                check(shift < 35 && offset < data.size) { "Invalid SentencePiece varint" }
                val byte = data[offset++].toInt() and 255
                value = value or ((byte and 127) shl shift)
                if (byte and 128 == 0) return value
                shift += 7
            } while (true)
        }
        fun bytes(): ByteArray {
            val size = varint()
            check(size >= 0 && size <= data.size - offset) { "Invalid SentencePiece length" }
            return data.copyOfRange(offset, offset + size).also { offset += size }
        }
        fun float(): Float {
            check(offset + 4 <= data.size) { "Invalid SentencePiece score" }
            return ByteBuffer.wrap(data, offset, 4).order(ByteOrder.LITTLE_ENDIAN).float.also { offset += 4 }
        }
        fun skip(wire: Int) {
            when (wire) {
                0 -> varint()
                1 -> offset += 8
                2 -> bytes()
                5 -> offset += 4
                else -> error("Unsupported SentencePiece wire type")
            }
            check(offset <= data.size) { "Truncated SentencePiece model" }
        }
    }

    fun export(model: ByteArray): String = buildString {
        val outer = Proto(model)
        while (outer.ready) {
            val tag = outer.varint()
            if (tag != 10) {
                outer.skip(tag and 7)
                continue
            }
            val inner = Proto(outer.bytes())
            var piece = ""
            var score = 0f
            while (inner.ready) {
                val field = inner.varint()
                when (field) {
                    10 -> piece = inner.bytes().toString(Charsets.UTF_8)
                    21 -> score = inner.float()
                    else -> inner.skip(field and 7)
                }
            }
            append(piece).append('\t').append(score.toDouble()).append('\n')
        }
        check(isNotEmpty()) { "Empty SentencePiece vocabulary" }
    }
}
