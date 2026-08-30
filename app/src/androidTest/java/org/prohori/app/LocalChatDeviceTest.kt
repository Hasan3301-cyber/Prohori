package org.prohori.app

import android.os.Bundle
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class LocalChatDeviceTest {
    @Test
    fun emergencyChatBypassesFreeFormModelAndSuggestsOnlyACareService() {
        val decision = emergencyChatDecision(Core.instance, "he has chest pain and is sweating")

        assertTrue("A deterministic red flag must produce an emergency decision", decision != null)
        assertTrue(decision!!.response.contains("will not diagnose"))
        assertTrue(decision.response.contains("routing category, not a diagnosis"))
        assertTrue(decision.response.contains("Source"))
        assertTrue(decision.target.specialty == "cardiac_emergency")
    }

    @Test
    fun localGenerationCanBeCancelledAfterItsFirstVisibleToken() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val model = ModelStore(instrumentation.targetContext).modelFile
        val firstVisibleToken = CountDownLatch(1)
        val executor = Executors.newSingleThreadExecutor()
        val future =
            executor.submit<Throwable?> {
                runCatching {
                    OnDeviceEngine.chat(
                        model,
                        listOf(
                            LocalChatTurn(
                                fromUser = true,
                                text = "Explain several simple ways to stay calm during a stressful day.",
                            ),
                        ),
                        onPartial = { firstVisibleToken.countDown() },
                    )
                }.exceptionOrNull()
            }

        try {
            assertTrue(
                "Local generation must expose a visible token before its deadline",
                firstVisibleToken.await(90, TimeUnit.SECONDS),
            )
            val cancelledAt = android.os.SystemClock.elapsedRealtime()
            OnDeviceEngine.cancel()
            val failure = future.get(15, TimeUnit.SECONDS)
            val cancelMillis = android.os.SystemClock.elapsedRealtime() - cancelledAt

            assertTrue(
                "Cancellation must terminate inference with an explicit result",
                failure?.message?.contains("cancel", ignoreCase = true) == true,
            )
            assertTrue("Native cancellation must stop promptly", cancelMillis <= 10_000)
        } finally {
            if (!future.isDone) OnDeviceEngine.cancel()
            executor.shutdownNow()
        }
    }

    @Test
    fun generalChatUsesHistoryWithoutRepeatingEmergencyBoilerplate() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val model = ModelStore(instrumentation.targetContext).modelFile
        val turns = mutableListOf<LocalChatTurn>()

        fun ask(text: String): LocalChatRun {
            turns += LocalChatTurn(fromUser = true, text = text)
            val run = OnDeviceEngine.chat(model, turns)
            turns += LocalChatTurn(fromUser = false, text = run.text)
            return run
        }

        val greeting = ask("hi")
        val concern = ask("i feel so much headache now")
        val followUp = ask("what to do now?")
        val genericWarning = "if this sounds like an emergency, call for help now"
        val stockClosing = "how can i assist you further"

        assertFalse("A greeting must not receive emergency boilerplate", greeting.text.lowercase().contains(genericWarning))
        assertFalse("A greeting must not use a stock closing", greeting.text.lowercase().contains(stockClosing))
        assertFalse("An ordinary headache must not receive generic boilerplate", concern.text.lowercase().contains(genericWarning))
        assertFalse("The follow-up must not repeat generic boilerplate", followUp.text.lowercase().contains(genericWarning))
        assertFalse("The follow-up must not use a stock closing", followUp.text.lowercase().contains(stockClosing))
        assertTrue("The headache answer must provide more than an echoed label", concern.text.trim().length > "Headache.".length)
        assertTrue("The contextual follow-up must provide a useful answer", followUp.text.split(Regex("\\s+")).size >= 6)

        instrumentation.sendStatus(
            2,
            Bundle().apply {
                putString(
                    "stream",
                    "PROHORI_CONTEXT_CHAT=" +
                        "greeting=${greeting.text.replace('\n', ' ')}," +
                        "concern=${concern.text.replace('\n', ' ')}," +
                        "follow_up=${followUp.text.replace('\n', ' ')}\\n",
                )
            },
        )
    }

    @Test
    fun bundledChatReturnsABoundedLocalAnswer() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val model = ModelStore(context).modelFile
        assertTrue("The bundled model must be prepared before chat", model.isFile)

        val firstTurn = LocalChatTurn(fromUser = true, text = "Hi.")
        val run = OnDeviceEngine.chat(model, listOf(firstTurn))

        assertFalse("Local chat must return visible text", run.text.isBlank())
        assertTrue("Chat must respect its native token cap", run.metrics.generatedTokens in 1..192)
        assertTrue("Chat must return or time out inside its device deadline", run.metrics.totalMillis <= 125_000)

        // The model remains loaded for the app process. A second turn is the normal chat
        // experience and must not pay the first-turn load/repack cost again.
        val warmRun =
            OnDeviceEngine.chat(
                model,
                listOf(
                    firstTurn,
                    LocalChatTurn(fromUser = false, text = run.text),
                    LocalChatTurn(fromUser = true, text = "Say hello briefly."),
                ),
            )
        assertFalse("Warm local chat must return visible text", warmRun.text.isBlank())
        assertTrue("Warm local chat should finish promptly", warmRun.metrics.totalMillis <= 15_000)
        instrumentation.sendStatus(
            2,
            Bundle().apply {
                putString(
                    "stream",
                    "PROHORI_CHAT_EVIDENCE=" +
                        "total_ms=${run.metrics.totalMillis}," +
                        "ttft_ms=${run.metrics.timeToFirstTokenMillis}," +
                        "tokens=${run.metrics.generatedTokens}," +
                        "tokens_per_second=${"%.2f".format(run.metrics.tokensPerSecond)}," +
                        "warm_total_ms=${warmRun.metrics.totalMillis}\n",
                )
            },
        )
    }

    @Test
    fun unmatchedSymptomGetsBoundedOfflineGuidance() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val model = ModelStore(context).modelFile
        val message = "my neighbour is trapped under a concrete slab"
        val core = Core.instance

        assertTrue("The test report must exercise the unmatched AI path", core.fallbackPermitted(message))
        val run = OnDeviceEngine.writeFallback(core, model, message)

        assertTrue(
            "Generated offline guidance was rejected: ${run.assessment.error}",
            run.assessment.accepted,
        )
        assertTrue("Accepted fallback must include visible guidance", run.assessment.guidance != null)
        assertTrue("Fallback must respect the native device deadline", run.metrics.totalMillis <= 185_000)
        instrumentation.sendStatus(
            2,
            Bundle().apply {
                putString(
                    "stream",
                    "PROHORI_FALLBACK_EVIDENCE=" +
                        "total_ms=${run.metrics.totalMillis}," +
                        "ttft_ms=${run.metrics.timeToFirstTokenMillis}," +
                        "tokens=${run.metrics.generatedTokens}\n",
                )
            },
        )
    }
}
