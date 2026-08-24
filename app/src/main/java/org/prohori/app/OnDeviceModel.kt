package org.prohori.app

import android.annotation.SuppressLint
import android.content.Context
import android.content.res.AssetManager
import android.net.Uri
import org.prohori.core.FallbackAssessment
import org.prohori.core.InferenceContract
import org.prohori.core.ModelAssessment
import org.prohori.core.Prohori
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.security.MessageDigest

data class ImportedModel(
    val file: File,
    val bytes: Long,
    val sha256: String,
)

internal const val BUNDLED_MODEL_ASSET = "models/qwen3-1.7b-q4_k_m.gguf"
internal const val BUNDLED_MODEL_BYTES = 1_107_408_544L
internal const val BUNDLED_MODEL_SHA256 = "54c0f1203a724e9f33e76916beab3bdfaffef56cf7b42a93b1bc21319fc0bf97"

data class InferenceMetrics(
    val modelLoadMillis: Long,
    val promptMillis: Long,
    val timeToFirstTokenMillis: Long,
    val totalMillis: Long,
    val generatedTokens: Long,
    val promptTokens: Long,
) {
    val tokensPerSecond: Double
        get() {
            val generationMillis = (totalMillis - timeToFirstTokenMillis).coerceAtLeast(1)
            return generatedTokens * 1_000.0 / generationMillis
        }
}

data class OnDeviceRun(
    val assessment: ModelAssessment,
    val metrics: InferenceMetrics,
)

/** One unmatched-query generation: what Rust made of it, and what it cost. */
data class FallbackRun(
    val assessment: FallbackAssessment,
    val metrics: InferenceMetrics,
)

data class LocalChatRun(
    val text: String,
    val metrics: InferenceMetrics,
)

/** App-private storage for the bundled or user-replaced GGUF; weights are never downloaded. */
class ModelStore(private val context: Context) {
    private val directory = File(context.filesDir, "models")
    val modelFile = File(directory, "qwen3-1.7b.gguf")

    fun installed(): Boolean =
        modelFile.isFile && modelFile.length() >= MIN_MODEL_BYTES && hasGgufMagic(modelFile)

    /**
     * Extract the APK's verified Q4 asset once. Android does not run application code during
     * package installation, and llama.cpp requires a normal path, so this happens behind the
     * first-launch preparation screen. Existing valid models survive app upgrades.
     */
    fun installBundled(): ImportedModel {
        if (installed()) {
            return ImportedModel(modelFile, modelFile.length(), "already-installed")
        }
        return installFromStream(
            openInput = {
                context.assets.open(BUNDLED_MODEL_ASSET, AssetManager.ACCESS_STREAMING)
            },
            expectedBytes = BUNDLED_MODEL_BYTES,
            expectedSha256 = BUNDLED_MODEL_SHA256,
        )
    }

    fun import(uri: Uri): ImportedModel =
        installFromStream(
            openInput = {
                requireNotNull(context.contentResolver.openInputStream(uri)) {
                    "The selected file could not be opened"
                }
            },
        )

    // This is an early friendly check only. FileOutputStream remains authoritative and its
    // failure is caught, cleaned up, and shown with Retry. Reserving cache space through
    // StorageManager would allow Android to evict unrelated applications' cache for a model.
    @SuppressLint("UsableSpace")
    private fun installFromStream(
        openInput: () -> InputStream,
        expectedBytes: Long? = null,
        expectedSha256: String? = null,
    ): ImportedModel {
        directory.mkdirs()
        require(directory.isDirectory) { "The private model directory could not be created" }
        expectedBytes?.let { bytes ->
            require(directory.usableSpace >= bytes + MIN_FREE_BYTES_AFTER_COPY) {
                "Not enough free storage to prepare the bundled AI model"
            }
        }
        val temporary = File(directory, "qwen3-1.7b.importing")
        val digest = MessageDigest.getInstance("SHA-256")
        var total = 0L
        try {
            openInput().use { input ->
                FileOutputStream(temporary, false).use { output ->
                    val buffer = ByteArray(COPY_BUFFER_BYTES)
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        total += read
                        require(total <= MAX_MODEL_BYTES) { "The selected file is larger than 2.5 GB" }
                        digest.update(buffer, 0, read)
                        output.write(buffer, 0, read)
                    }
                    output.fd.sync()
                }
            }
            require(total >= MIN_MODEL_BYTES) { "This is too small to be a Qwen3 1.7B model" }
            expectedBytes?.let { require(total == it) { "The bundled AI model has an unexpected size" } }
            require(hasGgufMagic(temporary)) { "The selected file is not a GGUF model" }
            val sha256 = digest.digest().toHex()
            expectedSha256?.let {
                require(sha256.equals(it, ignoreCase = true)) {
                    "The bundled AI model failed its integrity check"
                }
            }
            if (modelFile.exists() && !modelFile.delete()) error("Could not replace the previous model")
            check(temporary.renameTo(modelFile)) { "Could not finish installing the model" }
            return ImportedModel(modelFile, total, sha256)
        } catch (error: Throwable) {
            temporary.delete()
            throw error
        }
    }

    private fun hasGgufMagic(file: File): Boolean =
        runCatching {
            file.inputStream().use { input ->
                val magic = ByteArray(4)
                input.read(magic) == 4 && magic.contentEquals(byteArrayOf('G'.code.toByte(), 'G'.code.toByte(), 'U'.code.toByte(), 'F'.code.toByte()))
            }
        }.getOrDefault(false)

    private fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

    private companion object {
        const val MIN_MODEL_BYTES = 500_000_000L
        const val MAX_MODEL_BYTES = 2_500_000_000L
        const val MIN_FREE_BYTES_AFTER_COPY = 128_000_000L
        const val COPY_BUFFER_BYTES = 1024 * 1024
    }
}

/** JNI boundary for llama.cpp. All medical acceptance remains in the Rust verifier. */
object OnDeviceEngine {
    init {
        System.loadLibrary("prohori_llama")
    }

    fun assess(core: Prohori, model: File, message: String): ModelAssessment =
        assessWithMetrics(core, model, message).assessment

    /** P2: the model picks a protocol id and fills bounded fields. Rust decides urgency. */
    fun assessWithMetrics(core: Prohori, model: File, message: String): OnDeviceRun {
        val generated = generate(model, core.inferenceContract(message), message)
        return OnDeviceRun(
            assessment = core.acceptModelOutput(message, generated),
            metrics = lastMetrics(),
        )
    }

    /**
     * Let the model write guidance for a query the corpus does not cover.
     *
     * The only differences from [assessWithMetrics] are which contract is fetched and which
     * Rust function judges the result. Everything else — the chat template, the JNI entry
     * point, the 384-token cap — is shared on purpose: two templates for one model is how
     * one of them ends up subtly wrong on a phone nobody is holding.
     *
     * The safety of this path is not here. `core.fallbackContract` returns a grammar in which
     * a digit is unrepresentable, and `core.acceptFallbackOutput` re-checks the red-flag table
     * and retrieval before it looks at the text at all — so a red flag that fires while the
     * model is decoding discards what it wrote. This function cannot weaken either one: it
     * passes both strings straight through and never reads the JSON itself.
     */
    fun writeFallback(core: Prohori, model: File, message: String): FallbackRun {
        val report = message.trim()
        val generated = generate(model, core.fallbackContract(report), report)
        return FallbackRun(
            assessment = core.acceptFallbackOutput(report, generated),
            metrics = lastMetrics(),
        )
    }

    /** General-chat mode is local-only and deliberately has no hospital-routing tools. */
    fun chat(model: File, transcript: String): LocalChatRun {
        val cleaned = transcript.trim().takeLast(MAX_CHAT_INPUT_CHARS)
        require(cleaned.isNotEmpty()) { "Write a message first" }
        val prompt =
            "<|im_start|>system\n" +
                "Be brief and honest. Never diagnose. Emergencies: advise calling for help." +
                "<|im_end|>\n<|im_start|>user\n$cleaned /no_think<|im_end|>\n" +
                "<|im_start|>assistant\n"
        val generated =
            generateNative(
                model.absolutePath,
                prompt,
                "",
                CHAT_MAX_OUTPUT_TOKENS,
                CHAT_DEADLINE_MILLIS,
                true,
            ).trim()
        return LocalChatRun(generated, lastMetrics())
    }

    /** The Qwen chat template, written once, with the grammar the core chose. */
    private fun generate(model: File, contract: InferenceContract, message: String): String =
        generateNative(
            model.absolutePath,
            "<|im_start|>system\n${contract.prompt}<|im_end|>\n" +
                "<|im_start|>user\n${message.trim()} /no_think<|im_end|>\n" +
                "<|im_start|>assistant\n",
            contract.grammar,
            STRUCTURED_MAX_OUTPUT_TOKENS,
            STRUCTURED_DEADLINE_MILLIS,
            false,
        )

    private fun lastMetrics(): InferenceMetrics {
        val raw = lastMetricsNative()
        check(raw.size == 6) { "Native inference metrics are incomplete" }
        return InferenceMetrics(
            modelLoadMillis = raw[0] / 1_000,
            promptMillis = raw[1] / 1_000,
            timeToFirstTokenMillis = raw[2] / 1_000,
            totalMillis = raw[3] / 1_000,
            generatedTokens = raw[4],
            promptTokens = raw[5],
        )
    }

    private external fun generateNative(
        modelPath: String,
        prompt: String,
        grammar: String,
        maxOutputTokens: Int,
        deadlineMillis: Long,
        stopAfterSentence: Boolean,
    ): String

    private external fun lastMetricsNative(): LongArray

    private const val MAX_CHAT_INPUT_CHARS = 6_000
    private const val CHAT_MAX_OUTPUT_TOKENS = 32
    private const val CHAT_DEADLINE_MILLIS = 60_000L
    private const val STRUCTURED_MAX_OUTPUT_TOKENS = 384
    private const val STRUCTURED_DEADLINE_MILLIS = 180_000L

}
