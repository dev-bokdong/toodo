package dev.toodo.app

/** Pure transcript rules; offsets refer to characters, never whole BPE tokens. */
internal object WakeParser {
    data class Match(val end: Int, val at: Float)
    private const val cho = "\u3131\u3132\u3134\u3137\u3138\u3139\u3141\u3142\u3143\u3145\u3146\u3147\u3148\u3149\u314a\u314b\u314c\u314d\u314e"
    private const val jung = "\u314f\u3150\u3151\u3152\u3153\u3154\u3155\u3156\u3157\u3158\u3159\u315a\u315b\u315c\u315d\u315e\u315f\u3160\u3161\u3162\u3163"
    private val jong = listOf("", "\u3131", "\u3132", "\u3133", "\u3134", "\u3135", "\u3136", "\u3137", "\u3139", "\u313a", "\u313b", "\u313c", "\u313d", "\u313e", "\u313f", "\u3140", "\u3141", "\u3142", "\u3144", "\u3145", "\u3146", "\u3147", "\u3148", "\u314a", "\u314b", "\u314c", "\u314d", "\u314e")

    private fun pronounce(chars: List<Char>): List<String> {
        val parts = chars.map {
            val code = it.code - 0xac00
            arrayOf(cho[code / 588].toString(), jung[code % 588 / 28].toString(), jong[code % 28])
        }
        for (i in 0 until parts.lastIndex) {
            val a = parts[i]
            val b = parts[i + 1]
            if (a[2].length == 1 && b[0] == "\u3147") {
                b[0] = a[2]
                a[2] = ""
            }
        }
        return parts.map { (if (it[0] == "\u3147") "" else it[0]) + it[1] + it[2] }
    }

    private fun distance(a: String, b: String): Int {
        var previous = IntArray(b.length + 1) { it }
        a.forEachIndexed { i, ca ->
            val current = IntArray(b.length + 1)
            current[0] = i + 1
            b.forEachIndexed { j, cb ->
                current[j + 1] = minOf(previous[j + 1] + 1, current[j] + 1, previous[j] + if (ca == cb) 0 else 1)
            }
            previous = current
        }
        return previous.last()
    }

    fun find(tokens: Array<String>, times: FloatArray): Match? {
        data class Syllable(val char: Char, val offset: Int, val token: Int)
        val chars = mutableListOf<Syllable>()
        var offset = 0
        tokens.forEachIndexed { owner, token ->
            token.forEachIndexed { at, char ->
                if (char in '\uac00'..'\ud7a3') chars.add(Syllable(char, offset + at, owner))
            }
            offset += token.length
        }
        for (i in 0 until chars.size - 2) {
            val first = chars[i]
            val previous = chars.getOrNull(i - 1)
            val at = times.getOrNull(first.token) ?: continue
            // A match inside the previous spoken token cannot start a new utterance.
            if (previous != null && (previous.token == first.token ||
                    at - (times.getOrNull(previous.token) ?: at) < 0.5f)) continue
            val spoken = pronounce(chars.subList(i, i + 3).map { it.char })
            if (spoken[1].startsWith("\u3139\u3163") &&
                distance(spoken.joinToString(""), "\u3137\u3157\u3139\u3163\u3151") <= 2) {
                return Match(chars[i + 2].offset + 1, at)
            }
        }
        return null
    }

    fun message(tokens: Array<String>, match: Match): String =
        tokens.joinToString("").drop(match.end).replace('\u2581', ' ').trim(' ', ',', '.', '?', '!')
}

/** The caller feeds the same stream before and after activation, on a sample clock. */
internal class WakeCapture(private val began: Double, private val boundary: WakeParser.Match) {
    private var lastChange = began
    private var lastText = ""
    var text = ""
        private set

    fun update(tokens: Array<String>, times: FloatArray, now: Double): Boolean {
        // Beam-search may revise BPE segmentation, so locate the boundary again when possible.
        val revised = WakeParser.find(tokens, times)?.takeIf { kotlin.math.abs(it.at - boundary.at) <= 0.3f }
        text = WakeParser.message(tokens, revised ?: boundary)
        if (text != lastText) {
            lastText = text
            lastChange = now
        }
        return if (text.any { it.isLetterOrDigit() }) now - lastChange >= 2.0
        else now - began >= 5.0
    }
}

internal data class WakeConditions(
    val enabled: Boolean = false,
    val suspended: Boolean = false,
    val resumed: Boolean = false,
    val interactive: Boolean = false,
    val unlocked: Boolean = false,
    val manual: Boolean = false,
    val audioAvailable: Boolean = false,
    val permitted: Boolean = false,
    val pending: Boolean = false,
) {
    fun state(): String = when {
        pending -> "pending"
        !enabled -> "off"
        suspended || !resumed || !interactive || !unlocked || manual || !audioAvailable -> "paused"
        !permitted -> "error"
        else -> "listening"
    }
}

internal class WakeCooldown {
    private var until = 0L
    fun release(now: Long) { until = now + 300 }
    fun ready(now: Long): Boolean = now >= until
}
