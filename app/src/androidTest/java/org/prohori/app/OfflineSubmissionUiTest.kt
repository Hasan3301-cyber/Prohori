package org.prohori.app

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import org.junit.Rule
import org.junit.Assert.assertTrue
import org.junit.Test

class OfflineSubmissionUiTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun symptomFieldHasAVisibleWorkingSubmitButton() {
        var submitted = false
        compose.setContent {
            var message by remember { mutableStateOf("") }
            ProhoriTheme {
                SymptomInputCard(
                    message = message,
                    busy = false,
                    enabled = true,
                    onMessageChange = { message = it },
                    onSubmit = { submitted = true },
                )
            }
        }

        val input = compose.onNodeWithTag("offline_symptom_input")
        val submit = compose.onNodeWithTag("offline_submit")

        submit.assertIsDisplayed().assertIsNotEnabled()
        input.performTextInput("headache")
        submit.assertIsDisplayed().assertIsEnabled().performClick()
        compose.runOnIdle { assertTrue("The visible button must invoke submission", submitted) }
    }

    /**
     * A wait that is tens of seconds long has to say which part of itself it is in.
     *
     * The two long stages are not interchangeable: PREPARING can last most of the wait and
     * produces nothing, WRITING means text is arriving. Collapsing both into one spinner is the
     * failure this asserts against, so the test insists the two labels are visibly different.
     */
    @Test
    fun theSubmitButtonNamesTheStageItIsIn() {
        var stage by mutableStateOf(OfflineStage.CHECKING)
        compose.setContent {
            ProhoriTheme {
                SymptomInputCard(
                    message = "his leg is trapped under a slab",
                    busy = true,
                    enabled = true,
                    onMessageChange = {},
                    onSubmit = {},
                    stage = stage,
                )
            }
        }
        val submit = compose.onNodeWithTag("offline_submit")

        submit.assertTextContains("Checking warning signs", substring = true)
        compose.runOnIdle { stage = OfflineStage.PREPARING }
        submit.assertTextContains("AI is preparing guidance", substring = true)
        compose.runOnIdle { stage = OfflineStage.WRITING }
        submit.assertTextContains("AI is writing guidance", substring = true)
    }

    /** The end of the wait is announced too, and points at where the answer landed. */
    @Test
    fun aFinishedCheckSaysTheGuidanceIsReady() {
        compose.setContent {
            ProhoriTheme {
                SymptomInputCard(
                    message = "his leg is trapped under a slab",
                    busy = false,
                    enabled = true,
                    onMessageChange = {},
                    onSubmit = {},
                    stage = OfflineStage.READY,
                )
            }
        }
        compose.onNodeWithTag("offline_stage_ready").assertIsDisplayed()
        compose.onNodeWithTag("offline_submit").assertTextContains(
            "Check symptoms offline",
            substring = true,
        )
    }
}
