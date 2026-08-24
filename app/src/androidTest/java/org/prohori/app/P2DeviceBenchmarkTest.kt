package org.prohori.app

import android.os.Bundle
import android.os.Debug
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.prohori.core.Urgency
import java.io.File

@RunWith(AndroidJUnit4::class)
class P2DeviceBenchmarkTest {
    @Test
    fun constrainedInferenceProducesDeviceEvidence() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val model = File(context.getExternalFilesDir(null), "models/qwen3-1.7b.gguf")
        assertTrue("Push the Q4 model with tools/benchmark-p2-device.ps1", model.isFile)

        val cases =
            listOf(
                "he is not breathing" to "cpr.adult",
                "my father has chest pain and feels sweaty" to "chest.pain",
                // P0 deliberately over-triages conscious breathlessness to CPR.
                "cant breath properly" to "cpr.adult",
                "burn from hot water on my arm" to "burn.thermal",
                "she is awake after a seizure" to "seizure.active",
            )
        val results = JSONArray()
        cases.forEachIndexed { index, (message, expectedProtocol) ->
            val run = OnDeviceEngine.assessWithMetrics(Core.instance, model, message)
            assertTrue("model output must pass the Rust verifier", run.assessment.accepted)
            assertEquals(expectedProtocol, run.assessment.card?.protocolId)
            if (index == 0) assertEquals(Urgency.CRITICAL, run.assessment.severity)
            val memory = Debug.MemoryInfo().also(Debug::getMemoryInfo)
            results.put(
                JSONObject()
                    .put("case", index)
                    .put("model_load_ms", run.metrics.modelLoadMillis)
                    .put("prompt_ms", run.metrics.promptMillis)
                    .put("ttft_ms", run.metrics.timeToFirstTokenMillis)
                    .put("total_ms", run.metrics.totalMillis)
                    .put("generated_tokens", run.metrics.generatedTokens)
                    .put("tokens_per_second", run.metrics.tokensPerSecond)
                    .put("total_pss_bytes", memory.totalPss.toLong() * 1_024),
            )
        }
        val evidence =
            JSONObject()
                .put("schema_version", 1)
                .put("model_bytes", model.length())
                .put("runs", results)
                .toString()
        instrumentation.sendStatus(
            2,
            Bundle().apply { putString("stream", "PROHORI_P2_EVIDENCE=$evidence\n") },
        )
    }
}
