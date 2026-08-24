package org.prohori.app

import android.os.Bundle
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LocalChatDeviceTest {
    @Test
    fun bundledChatReturnsABoundedLocalAnswer() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val model = ModelStore(context).modelFile
        assertTrue("The bundled model must be prepared before chat", model.isFile)

        val run = OnDeviceEngine.chat(model, "User: Hi.")

        assertFalse("Local chat must return visible text", run.text.isBlank())
        assertTrue("Chat must respect its native token cap", run.metrics.generatedTokens in 1..32)
        assertTrue("Chat must return or time out inside its device deadline", run.metrics.totalMillis <= 65_000)
        instrumentation.sendStatus(
            2,
            Bundle().apply {
                putString(
                    "stream",
                    "PROHORI_CHAT_EVIDENCE=" +
                        "total_ms=${run.metrics.totalMillis}," +
                        "ttft_ms=${run.metrics.timeToFirstTokenMillis}," +
                        "tokens=${run.metrics.generatedTokens}," +
                        "tokens_per_second=${"%.2f".format(run.metrics.tokensPerSecond)}\n",
                )
            },
        )
    }
}
