package dev.toodo.app

import com.k2fsa.sherpa.onnx.EndpointConfig
import com.k2fsa.sherpa.onnx.EndpointRule
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OnlineModelConfig
import com.k2fsa.sherpa.onnx.OnlineRecognizer
import com.k2fsa.sherpa.onnx.OnlineRecognizerConfig
import com.k2fsa.sherpa.onnx.OnlineTransducerModelConfig

/** Worker-owned native resources. No platform recognizer or second PCM consumer. */
internal class WakeRecognizer(private val directory: String) : AutoCloseable {
    private val recognizer = OnlineRecognizer(
        config = OnlineRecognizerConfig(
            featConfig = FeatureConfig(sampleRate = 16000, featureDim = 80),
            modelConfig = OnlineModelConfig(
                transducer = OnlineTransducerModelConfig(
                    encoder = "$directory/encoder-epoch-99-avg-1.int8.onnx",
                    decoder = "$directory/decoder-epoch-99-avg-1.onnx",
                    joiner = "$directory/joiner-epoch-99-avg-1.int8.onnx",
                ),
                tokens = "$directory/tokens.txt",
                numThreads = 1,
                modelType = "zipformer",
                modelingUnit = "bpe",
                bpeVocab = "$directory/bpe.vocab",
            ),
            endpointConfig = EndpointConfig(
                rule1 = EndpointRule(false, 2.4f, 0.0f),
                rule2 = EndpointRule(true, 0.8f, 0.0f),
                rule3 = EndpointRule(false, 0.0f, 20.0f),
            ),
            decodingMethod = "modified_beam_search",
            hotwordsFile = "$directory/toodo-hotwords.txt",
            hotwordsScore = 2.0f,
        ),
    )
    private var stream = recognizer.createStream()
    private var fed = 0L
    private var capture: WakeCapture? = null
    val capturing: Boolean get() = capture != null

    /** null means keep listening; empty means cancel an empty capture. */
    fun feed(samples: FloatArray): String? {
        stream.acceptWaveform(samples, 16000)
        fed += samples.size
        while (recognizer.isReady(stream)) recognizer.decode(stream)
        var result = recognizer.getResult(stream)
        if (capture == null) {
            WakeParser.find(result.tokens, result.timestamps)?.let {
                capture = WakeCapture(fed / 16000.0, it)
            }
        }
        val current = capture
        if (current != null) {
            if (current.update(result.tokens, result.timestamps, fed / 16000.0)) {
                stream.inputFinished()
                while (recognizer.isReady(stream)) recognizer.decode(stream)
                result = recognizer.getResult(stream)
                current.update(result.tokens, result.timestamps, fed / 16000.0)
                val text = current.text.takeIf { text -> text.any { it.isLetterOrDigit() } } ?: ""
                if (text.isEmpty()) {
                    stream.release()
                    stream = recognizer.createStream()
                    capture = null
                }
                return text
            }
        } else if (recognizer.isEndpoint(stream)) {
            recognizer.reset(stream)
        }
        return null
    }

    override fun close() {
        stream.release()
        recognizer.release()
    }
}
