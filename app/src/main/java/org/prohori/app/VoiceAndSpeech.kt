package org.prohori.app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.speech.RecognizerIntent
import android.speech.tts.TextToSpeech
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import java.util.Locale

/** Uses the phone's configured speech service; Prohori receives text, never microphone audio. */
@Composable
fun VoiceInputButton(
    enabled: Boolean,
    prompt: String,
    onText: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    var error by remember { mutableStateOf<String?>(null) }
    val launcher =
        rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            if (result.resultCode == Activity.RESULT_OK) {
                val spoken = result.data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)?.firstOrNull()
                if (!spoken.isNullOrBlank()) onText(spoken.trim())
            }
        }
    OutlinedButton(
        onClick = {
            error = null
            val intent =
                Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                    putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                    putExtra(RecognizerIntent.EXTRA_LANGUAGE, Locale.getDefault().toLanguageTag())
                    putExtra(RecognizerIntent.EXTRA_PROMPT, prompt)
                    putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
                }
            try {
                launcher.launch(intent)
            } catch (_: ActivityNotFoundException) {
                error = context.getString(R.string.voice_unavailable)
            }
        },
        enabled = enabled,
        modifier = modifier,
    ) { Text(error ?: stringResource(R.string.speak_instead), maxLines = 2) }
}

/** User-triggered platform TTS. It never starts automatically during an emergency. */
@Composable
fun ReadAloudControls(text: String, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    var ready by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    val speech =
        remember(context) {
            TextToSpeech(context.applicationContext) { status ->
                ready = status == TextToSpeech.SUCCESS
                failed = status != TextToSpeech.SUCCESS
            }
        }
    DisposableEffect(speech) {
        onDispose {
            speech.stop()
            speech.shutdown()
        }
    }
    Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton(
            onClick = {
                val language = if (text.any { it in '\u0980'..'\u09FF' }) Locale.forLanguageTag("bn-BD") else Locale.getDefault()
                speech.language = language
                speech.speak(text, TextToSpeech.QUEUE_FLUSH, null, "prohori-visible-guidance")
            },
            enabled = ready && text.isNotBlank(),
            modifier = Modifier.weight(1f),
        ) { Text(if (failed) stringResource(R.string.read_aloud_unavailable) else stringResource(R.string.read_aloud)) }
        TextButton(onClick = { speech.stop() }, enabled = ready) { Text(stringResource(R.string.stop)) }
    }
}
