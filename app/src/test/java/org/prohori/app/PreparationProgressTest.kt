package org.prohori.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The first-launch copy is the longest wait in the app and the first thing anyone sees.
 *
 * These tests exist because the failure they guard against is not a crash. A remaining-time
 * estimate that climbs, or that is offered confidently from the first fraction of a percent,
 * teaches a first-time user that this app's statements about itself are unreliable — and the
 * next screen asks them to trust it about someone who is bleeding.
 */
class PreparationProgressTest {
    private val total = BUNDLED_MODEL_BYTES

    @Test
    fun nothing_copied_yet_makes_no_claim() {
        assertEquals("Starting…", preparationProgressLabel(0, total, 0))
        assertEquals("Starting…", preparationProgressLabel(0, total, 30))
    }

    @Test
    fun an_estimate_waits_until_there_is_something_to_estimate_from() {
        // One second in and four megabytes deep, the honest answer is "I do not know yet".
        val early = preparationProgressLabel(4_000_000, total, 1)
        assertTrue(early, early.endsWith("estimating"))
        assertTrue(early, early.startsWith("4 MB of 1107 MB"))
    }

    @Test
    fun a_steady_copy_reports_a_shrinking_estimate() {
        // A modest phone hashing and writing at ~8 MB/s, which is where the multi-minute wait
        // this screen exists for actually happens. The same rate sampled later must never
        // read longer than it did before.
        val rate = 8_000_000L
        var previous = Int.MAX_VALUE
        var seen = 0
        for (seconds in 4..130) {
            val label = preparationProgressLabel(rate * seconds, total, seconds)
            val minutes = Regex("about (\\d+) min").find(label)?.groupValues?.get(1)?.toInt()
            if (minutes != null) {
                assertTrue("estimate grew at ${seconds}s: $label", minutes <= previous)
                previous = minutes
                seen += 1
            }
        }
        assertTrue("a minute estimate was never produced", seen > 0)
        assertEquals("the estimate never counted down to its last minute", 1, previous)
    }

    @Test
    fun the_last_stretch_says_less_than_a_minute_rather_than_zero() {
        // 1050 MB done in 30 s is 35 MB/s, so ~57 MB remain: under a minute, not "0 min".
        val label = preparationProgressLabel(1_050_000_000, total, 30)
        assertEquals("1050 MB of 1107 MB · less than a minute left", label)
    }

    @Test
    fun a_finished_copy_does_not_promise_more_waiting() {
        assertEquals("1107 MB of 1107 MB · finishing", preparationProgressLabel(total, total, 40))
    }

    @Test
    fun impossible_inputs_cannot_produce_a_wrong_number_or_a_crash() {
        // A negative or over-long count would come from a bug elsewhere; it must not become a
        // divide by zero or a claim that more than the whole model has been copied.
        assertEquals("Starting…", preparationProgressLabel(-500, total, 10))
        assertEquals("1107 MB of 1107 MB · finishing", preparationProgressLabel(total * 3, total, 10))
        assertEquals("Starting…", preparationProgressLabel(0, 0, 10))
    }
}
