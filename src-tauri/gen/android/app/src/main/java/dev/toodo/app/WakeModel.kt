package dev.toodo.app

import java.io.File
import java.io.InterruptedIOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/** Download on first opt-in, never redistribute the model in the APK. */
internal object WakeModel {
    private const val base = "https://huggingface.co/k2-fsa/sherpa-onnx-streaming-zipformer-korean-2024-06-16/resolve/ba6078bca4daf3f0dd37f79d0ab505af71df14a6"
    val files = linkedMapOf(
        "encoder-epoch-99-avg-1.int8.onnx" to "8d0b1aa24fbedd4e3948564ab7facd151b8ce9b0c48fc987c541de2de3af5697",
        "decoder-epoch-99-avg-1.onnx" to "b29cfb4575141e50a30a22b2c4579934f3d4f45b83c9c8c08c3aef5a3fa7abfc",
        "joiner-epoch-99-avg-1.int8.onnx" to "128b80a66a1f718488af8560f9d15895109b99ff3e573f0a0130e03774ef1ced",
        "tokens.txt" to "016bdf0965029263b7ad01b742366ee542ef0bef38261510e8176ff6f2e9e668",
        "bpe.model" to "1491bc92c47dfda4225f5b8930fba3cfa34c3b1ccd25e7d96c630a262f3e918d",
    )

    private fun hash(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(65536)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                digest.update(buffer, 0, count)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it.toInt() and 255) }
    }

    fun prepare(root: File, allowed: () -> Boolean): File {
        fun checkAllowed() {
            if (!allowed()) throw InterruptedIOException("Wake preparation cancelled")
        }
        val directory = File(root, "wake-korean-2024-06-16")
        check(directory.isDirectory || directory.mkdirs()) { "Could not create private model directory" }
        for ((name, checksum) in files) {
            checkAllowed()
            val target = File(directory, name)
            if (target.isFile && hash(target) == checksum) continue
            val partial = File(directory, "$name.part")
            val connection = URL("$base/$name").openConnection() as HttpURLConnection
            connection.connectTimeout = 15000
            connection.readTimeout = 15000
            try {
                check(connection.responseCode == 200) { "Model download failed (${connection.responseCode})" }
                connection.inputStream.use { input ->
                    partial.outputStream().use { output ->
                        val buffer = ByteArray(65536)
                        while (true) {
                            checkAllowed()
                            val count = input.read(buffer)
                            if (count < 0) break
                            output.write(buffer, 0, count)
                        }
                    }
                }
                check(hash(partial) == checksum) { "Model checksum mismatch: $name" }
                checkAllowed()
                check(partial.renameTo(target)) { "Could not install verified model: $name" }
            } finally {
                connection.disconnect()
                partial.delete()
            }
        }
        checkAllowed()
        // Generate only after every input is verified. No recognizer sees partial files.
        File(directory, "bpe.vocab").writeText(WakeVocabulary.export(File(directory, "bpe.model").readBytes()))
        File(directory, "toodo-hotwords.txt").writeText("\ub3c4\ub9ac\uc57c\n")
        return directory
    }
}
