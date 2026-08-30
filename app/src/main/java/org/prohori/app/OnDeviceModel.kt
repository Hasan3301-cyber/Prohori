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

/** One line of the local conversation, in the order it happened. */
data class LocalChatTurn(
    val fromUser: Boolean,
    val text: String,
)

/**
 * Receives the answer while the model is still writing it.
 *
 * Called from C++ on the thread that is running the decode, once per token that completes a
 * character, and it holds the native engine lock while it runs. So an implementation may only
 * record what it is given: it must not call back into [OnDeviceEngine], must not block, and
 * must be safe to touch from a background thread.
 *
 * This exists because a decode on a mid-range phone takes tens of seconds, and a spinner held
 * for that long is indistinguishable from a hang. Someone waiting on first-aid guidance
 * deserves proof the phone is working, and growing text is that proof.
 */
fun interface TokenSink {
    fun onToken(piece: String)
}

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
     *
     * [onProgress] receives bytes copied so far. This is the longest wait in the app — over a
     * gigabyte, copied and hashed in one pass on a phone — and it happens before anyone has
     * seen the app work, so it is the wait most likely to be read as a hang and answered by
     * uninstalling. The byte count already exists; the only defect was not showing it.
     */
    fun installBundled(onProgress: (Long) -> Unit = {}): ImportedModel {
        if (installed()) {
            return ImportedModel(modelFile, modelFile.length(), "already-installed")
        }
        return installFromStream(
            openInput = {
                context.assets.open(BUNDLED_MODEL_ASSET, AssetManager.ACCESS_STREAMING)
            },
            expectedBytes = BUNDLED_MODEL_BYTES,
            expectedSha256 = BUNDLED_MODEL_SHA256,
            onProgress = onProgress,
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
        onProgress: (Long) -> Unit = {},
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
                        // Once per megabyte, from whichever thread is doing the copy. The
                        // hash is folded into this same pass, so the count is the whole job
                        // and an estimate built from it does not stall at the end.
                        onProgress(total)
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

    /** Requests cancellation of the one serialized native generation, if any. */
    fun cancel() = cancelNative()

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
     *
     * [onProgress] reports how many characters of JSON the model has emitted. That number is
     * not for display: it is there so the screen can tell the two halves of a long wait apart.
     * Zero means the phone is still reading the prompt, which on a mid-range device is most of
     * the wait; anything above zero means guidance is being written. A single spinner covers
     * both and so tells the person holding the phone nothing.
     */
    fun writeFallback(
        core: Prohori,
        model: File,
        message: String,
        onProgress: (Int) -> Unit = {},
    ): FallbackRun {
        val report = message.trim()
        var characters = 0
        val generated =
            generate(
                model,
                core.fallbackContract(report),
                report,
                TokenSink { piece ->
                    characters += piece.length
                    onProgress(characters)
                },
            )
        return FallbackRun(
            assessment = core.acceptFallbackOutput(report, generated),
            metrics = lastMetrics(),
        )
    }

    /**
     * General-chat mode is local-only and deliberately has no hospital-routing tools.
     *
     * Two things here are load-bearing and were both wrong before.
     *
     * First, the reasoning switch. Qwen3 is a hybrid reasoning model, and `/no_think` in the
     * user turn is only a *soft* request: the model may honour it, may ignore it, and either
     * way it opens a `<think>` block that this app renders as visible text, because
     * [prohori_llama.cpp]'s `append_piece` asks `llama_token_to_piece` for special tokens.
     * Qwen's own template disables reasoning *structurally* instead, by opening the assistant
     * turn with an already-closed block. That is what [EMPTY_REASONING] does. Without it the
     * first thing the user reads is `<think>`, and when the model actually reasons, the
     * sentence stop fires on a full stop inside the block and the reply is a fragment of
     * reasoning with no answer in it at all.
     *
     * Second, the transcript. The caller's turns are written as real `<|im_start|>` turns.
     * Flattening them into one user turn with `User:` / `Assistant:` labels asks the model to
     * infer a structure it was trained to be *given*, and invites it to continue the
     * transcript rather than answer.
     *
     * [onPartial] is handed the whole answer *so far*, not the newest fragment, and only ever
     * the answer: a `<think>` block the model opens on its own is withheld until it closes,
     * at which point the reasoning is replaced by the real answer rather than appended to.
     * Publishing whole snapshots is what makes that replacement possible, so callers should
     * assign the value rather than concatenate it. See [answerOrNull].
     */
    fun chat(
        model: File,
        history: List<LocalChatTurn>,
        onPartial: (String) -> Unit = {},
    ): LocalChatRun {
        val turns = recentTurns(history)
        require(turns.isNotEmpty()) { "Write a message first" }
        val prompt =
            buildString {
                append("<|im_start|>system\n").append(CHAT_SYSTEM_PROMPT).append("<|im_end|>\n")
                turns.forEach { turn ->
                    append(if (turn.fromUser) "<|im_start|>user\n" else "<|im_start|>assistant\n")
                    append(turn.text).append("<|im_end|>\n")
                }
                append("<|im_start|>assistant\n").append(EMPTY_REASONING)
            }
        val raw = StringBuilder()
        var published = ""
        val generated =
            generateNative(
                model.absolutePath,
                prompt,
                "",
                CHAT_MAX_OUTPUT_TOKENS,
                CHAT_DEADLINE_MILLIS,
                true,
                TokenSink { piece ->
                    raw.append(piece)
                    val answer = answerOrNull(raw.toString())
                    if (answer != null && answer != published) {
                        published = answer
                        onPartial(answer)
                    }
                },
            )
        return LocalChatRun(answerOnly(generated), lastMetrics())
    }

    /**
     * Keep the newest turns that fit the input budget, oldest dropped first.
     *
     * The last turn always survives even when it alone exceeds the budget, because that is
     * the question being asked; it is truncated instead. Prior assistant turns carry no
     * reasoning block, matching Qwen's template, which strips reasoning from history.
     *
     * The budget is small on purpose, and it is the main lever this app has over how long an
     * answer takes. Every call frees its llama context when it returns, so the entire prompt
     * is tokenised and prefilled again on every single turn — history is not carried in a
     * cache, it is re-read from the start each time. Prefill is most of the wait on a phone,
     * so at the old six-thousand-character budget a long conversation spent tens of seconds
     * re-reading itself before it wrote a word, and the cost grew with every exchange.
     *
     * The alternative — keeping the context and its KV cache alive between calls, and reusing
     * the common prefix — is the faster design and is deliberately not here. `n_ctx` is sized
     * per request precisely so a two-word message does not reserve a full-length cache, and a
     * context held open between turns holds that memory for as long as the app is running.
     * Trading a low-memory phone's stability for a few seconds is the wrong trade in an app
     * someone opens during an emergency. See the sizing comment in `prohori_llama.cpp`.
     */
    private fun recentTurns(history: List<LocalChatTurn>): List<LocalChatTurn> {
        val spoken =
            history.mapNotNull { turn ->
                val text = answerOrNull(turn.text) ?: return@mapNotNull null
                LocalChatTurn(turn.fromUser, text.takeLast(MAX_CHAT_TURN_CHARS))
            }
        val kept = ArrayDeque<LocalChatTurn>()
        var characters = 0
        for (turn in spoken.asReversed()) {
            if (kept.size >= MAX_CHAT_TURNS) break
            if (kept.isNotEmpty() && characters + turn.text.length > MAX_CHAT_HISTORY_CHARS) break
            kept.addFirst(turn)
            characters += turn.text.length
        }
        return kept.toList()
    }

    /**
     * Belt and braces over [EMPTY_REASONING]. The prefilled block should make this a no-op,
     * but a fine-tuned adapter or a future model can open a block of its own, and reasoning
     * must never reach a frightened reader dressed as the answer.
     */
    private fun answerOnly(generated: String): String =
        checkNotNull(answerOrNull(generated)) {
            "The model reasoned instead of answering. Ask again in fewer words."
        }

    private fun answerOrNull(text: String): String? {
        val closed = text.lastIndexOf(REASONING_CLOSE)
        val answer =
            when {
                closed >= 0 -> text.substring(closed + REASONING_CLOSE.length)
                text.contains(REASONING_OPEN) -> text.substringBefore(REASONING_OPEN)
                else -> text
            }
        return removeStockClosing(answer.trim()).ifEmpty { null }
    }

    /**
     * Small local models do not always obey a negative prompt. Remove only known generic
     * closings at the very end of a response; never rewrite substantive content in the middle.
     */
    private fun removeStockClosing(answer: String): String =
        answer.replace(STOCK_CLOSING, "").trimEnd(' ', '\n', '\r', '\t', '-', '–', '—')

    /** The Qwen chat template, written once, with the grammar the core chose. */
    private fun generate(
        model: File,
        contract: InferenceContract,
        message: String,
        sink: TokenSink? = null,
    ): String =
        generateNative(
            model.absolutePath,
            "<|im_start|>system\n${contract.prompt}<|im_end|>\n" +
                "<|im_start|>user\n${message.trim()}<|im_end|>\n" +
                "<|im_start|>assistant\n" + EMPTY_REASONING,
            contract.grammar,
            STRUCTURED_MAX_OUTPUT_TOKENS,
            STRUCTURED_DEADLINE_MILLIS,
            false,
            sink,
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
        sink: TokenSink?,
    ): String

    private external fun cancelNative()

    private external fun lastMetricsNative(): LongArray

    // The question being asked is never trimmed below what the input field accepts. The
    // history budget is separate and much smaller, because it is paid again on every turn.
    private const val MAX_CHAT_TURN_CHARS = 2_000
    private const val MAX_CHAT_HISTORY_CHARS = 1_600
    private const val MAX_CHAT_TURNS = 6

    /**
     * Qwen3's non-thinking mode, expressed structurally. The block is opened and closed with
     * nothing between it, exactly as Qwen's published template does when reasoning is off.
     */
    private const val EMPTY_REASONING = "<think>\n\n</think>\n\n"
    private const val REASONING_OPEN = "<think>"
    private const val REASONING_CLOSE = "</think>"
    private val STOCK_CLOSING =
        Regex(
            "(?i)(?:how can i assist you further|how else can i help(?: you)?|" +
                "is there anything else i can help you with)\\?\\s*$",
        )

    private const val CHAT_SYSTEM_PROMPT =
        "You are Prohori's private general assistant. Answer the user's actual question " +
            "naturally and use the conversation history. Give a helpful answer instead of " +
            "merely repeating the user's words. For an ordinary health concern, suggest " +
            "simple low-risk self-care and ask one specific question about the concern when " +
            "needed. Do not reuse a stock sentence or closing from an earlier reply, and " +
            "never say 'How can I assist you further?'. " +
            "Do not diagnose, name a medicine, or give a dose. Do not append a generic " +
            "emergency warning to every reply. Recommend urgent help only when the user " +
            "describes a clear danger sign such as trouble breathing, unconsciousness, " +
            "sudden weakness, severe bleeding, or a sudden worst-ever headache. For a " +
            "greeting, simply greet them. Use two to four concise sentences."

    // Thirty-two tokens cut every answer mid-sentence — the reported "no response" was often a
    // truncated fragment. The native sentence stop still ends a finished answer early, so this
    // is a ceiling for a rambling model, not a target, and the deadline moves with it.
    private const val CHAT_MAX_OUTPUT_TOKENS = 192
    private const val CHAT_DEADLINE_MILLIS = 120_000L
    private const val STRUCTURED_MAX_OUTPUT_TOKENS = 384
    private const val STRUCTURED_DEADLINE_MILLIS = 180_000L
}
