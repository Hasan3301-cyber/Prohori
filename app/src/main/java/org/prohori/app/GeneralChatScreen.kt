package org.prohori.app

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.prohori.core.Prohori

private data class ChatLine(val fromUser: Boolean, val text: String)

@Composable
fun GeneralChatScreen(
    core: Prohori,
    onOpenEmergency: () -> Unit = {},
    onFindHospitals: (String) -> Unit = {},
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember { ModelStore(context.applicationContext) }
    val messages = remember { mutableStateListOf<ChatLine>() }
    var installed by remember { mutableStateOf(store.installed()) }
    var input by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf<String?>(null) }
    var elapsedSeconds by remember { mutableIntStateOf(0) }
    var requestId by remember { mutableIntStateOf(0) }
    var canRetry by remember { mutableStateOf(false) }
    var emergencyTarget by remember { mutableStateOf<EmergencyCareTarget?>(null) }

    // The model writes from a background thread while it holds the native engine lock, so the
    // live answer arrives through a flow rather than straight into snapshot state.
    val liveAnswer = remember { MutableStateFlow("") }
    val live by liveAnswer.collectAsState()

    LaunchedEffect(busy) {
        elapsedSeconds = 0
        while (busy) {
            delay(1_000)
            elapsedSeconds += 1
        }
    }

    /**
     * Ask the model about the conversation as it stands.
     *
     * Deliberately reads [messages] rather than taking the question as an argument, so that
     * Try again and Send are the same code path. After a failure the last line is still the
     * unanswered question, which is exactly the history a retry needs.
     */
    val ask: () -> Unit = {
        val activeRequest = ++requestId
        busy = true
        canRetry = false
        liveAnswer.value = ""
        note = null
        scope.launch {
            try {
                val history = messages.map { LocalChatTurn(it.fromUser, it.text) }
                val result =
                    runCatching {
                        withContext(Dispatchers.Default) {
                            OnDeviceEngine.chat(store.modelFile, history) { partial ->
                                if (activeRequest == requestId) liveAnswer.value = partial
                            }
                        }
                    }
                if (activeRequest == requestId) {
                    result.onSuccess { run ->
                        messages += ChatLine(false, run.text)
                        val seconds = (run.metrics.totalMillis + 999) / 1_000
                        note = "Answered privately on this phone in ${seconds}s."
                    }.onFailure {
                        note = it.message ?: "The local model could not answer in time."
                        canRetry = true
                    }
                    liveAnswer.value = ""
                }
            } finally {
                if (activeRequest == requestId) busy = false
            }
        }
    }

    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri != null) {
                scope.launch {
                    busy = true
                    val result = runCatching { withContext(Dispatchers.IO) { store.import(uri) } }
                    installed = store.installed()
                    note = result.fold({ "Local model installed." }, { it.message ?: "Model import failed." })
                    busy = false
                }
            }
        }

    Column(Modifier.fillMaxSize().padding(horizontal = 20.dp, vertical = 22.dp)) {
        Text(stringResource(R.string.chat_private_ai), style = MaterialTheme.typography.labelMedium, color = ProhoriGreen)
        Spacer(Modifier.height(6.dp))
        Text(stringResource(R.string.chat_title), style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(8.dp))
        Text(
            stringResource(R.string.chat_scope),
            style = MaterialTheme.typography.bodyMedium,
            color = ProhoriMuted,
        )
        Spacer(Modifier.height(6.dp))
        // Said plainly, and said here rather than in a help screen nobody opens. The private
        // model is a fraction of the size of a cloud one, and someone who expects cloud-grade
        // answers will read a thin answer as the app being broken instead of the model being
        // small. Online mode is one tap away and is the honest thing to point at.
        Text(
            "This small private model is far less capable than a cloud AI. It can be brief or " +
                "wrong. Use it for simple general suggestions, not diagnosis or urgent decisions.",
            style = MaterialTheme.typography.bodySmall,
            color = ProhoriMuted,
        )
        Spacer(Modifier.height(14.dp))
        Surface(
            color = ProhoriWhite,
            shape = RoundedCornerShape(13.dp),
            border = BorderStroke(1.dp, ProhoriBorder),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 11.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Surface(modifier = Modifier.size(9.dp), shape = RoundedCornerShape(50), color = ProhoriGreen) {}
                Text(
                    if (installed) "LOCAL MODEL READY" else "LOCAL MODEL REQUIRED",
                    modifier = Modifier.padding(start = 9.dp).weight(1f),
                    style = MaterialTheme.typography.labelMedium,
                )
                Text("No cloud", style = MaterialTheme.typography.bodySmall, color = ProhoriMuted)
            }
        }
        if (!installed) {
            OutlinedButton(
                onClick = { picker.launch(arrayOf("application/octet-stream", "*/*")) },
                enabled = !busy,
                modifier = Modifier.fillMaxWidth().padding(top = 10.dp),
            ) { Text("Install GGUF model") }
        }
        note?.let {
            Surface(
                modifier = Modifier.fillMaxWidth().padding(top = 10.dp),
                color = MaterialTheme.colorScheme.secondaryContainer,
                shape = RoundedCornerShape(10.dp),
            ) { Text(it, modifier = Modifier.padding(10.dp), style = MaterialTheme.typography.bodySmall) }
        }
        emergencyTarget?.let { target ->
            Surface(
                modifier = Modifier.fillMaxWidth().padding(top = 10.dp),
                color = ProhoriGreenSoft,
                shape = RoundedCornerShape(12.dp),
                border = BorderStroke(1.dp, ProhoriGreen.copy(alpha = 0.3f)),
            ) {
                Column(Modifier.padding(12.dp)) {
                    Text("EMERGENCY HANDOFF", style = MaterialTheme.typography.labelMedium, color = ProhoriRed)
                    Text(
                        "Suggested service: ${target.userLabel}. This is a routing category, not a diagnosis.",
                        style = MaterialTheme.typography.bodySmall,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        TextButton(onClick = onOpenEmergency) { Text("Open first aid") }
                        TextButton(onClick = { onFindHospitals(target.specialty) }) { Text("Find hospitals") }
                    }
                }
            }
        }
        LazyColumn(
            modifier = Modifier.weight(1f).fillMaxWidth().padding(vertical = 14.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (messages.isEmpty()) {
                item {
                    Surface(
                        modifier = Modifier.fillMaxWidth(),
                        color = ProhoriWhite,
                        shape = RoundedCornerShape(18.dp),
                        border = BorderStroke(1.dp, ProhoriBorder),
                    ) {
                        Column(Modifier.padding(18.dp)) {
                            Text("Start a conversation", style = MaterialTheme.typography.titleMedium)
                            Text(
                                "Ask for a simple explanation, preparation checklist, or a general suggestion.",
                                style = MaterialTheme.typography.bodySmall,
                                color = ProhoriMuted,
                            )
                            TextButton(onClick = { input = "How can I prepare a basic emergency kit?" }) {
                                Text("Try: prepare an emergency kit")
                            }
                        }
                    }
                }
            }
            items(messages) { line ->
                ChatBubble(fromUser = line.fromUser, text = line.text)
            }
            if (busy) {
                item {
                    ChatBubble(
                        fromUser = false,
                        text = live.ifEmpty { "Reading your message…" },
                        label = if (live.isEmpty()) "LOCAL AI · PREPARING" else "LOCAL AI · WRITING",
                        muted = live.isEmpty(),
                    )
                }
            }
        }
        OutlinedTextField(
            value = input,
            onValueChange = { input = it.take(2_000) },
            label = { Text(stringResource(R.string.chat_input)) },
            placeholder = { Text(stringResource(R.string.chat_placeholder)) },
            enabled = !busy,
            minLines = 2,
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(8.dp))
        VoiceInputButton(
            enabled = !busy,
            prompt = "Ask the local assistant",
            onText = { input = it.take(2_000) },
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(8.dp))
        Button(
            enabled = input.isNotBlank() && !busy,
            modifier = Modifier.fillMaxWidth().height(52.dp),
            colors = ButtonDefaults.buttonColors(containerColor = ProhoriInk),
            onClick = {
                val question = input.trim()
                input = ""
                messages += ChatLine(true, question)
                // A short follow-up such as "what now?" must retain the emergency context.
                // Only user messages are included and none of this text is persisted.
                val contextText =
                    messages.filter { it.fromUser }.takeLast(4).joinToString("\n") { it.text }
                val emergency = emergencyChatDecision(core, contextText)
                if (emergency != null) {
                    emergencyTarget = emergency.target
                    canRetry = false
                    messages += ChatLine(false, emergency.response)
                    note = "Deterministic emergency rules answered; the language model was bypassed."
                } else if (installed) {
                    emergencyTarget = null
                    ask()
                } else {
                    note = "Install the local model for general chat. Emergency rules remain available without it."
                }
            },
        ) {
            if (busy) {
                CircularProgressIndicator(
                    modifier = Modifier.size(19.dp),
                    strokeWidth = 2.dp,
                    color = ProhoriWhite,
                )
                Spacer(Modifier.size(9.dp))
            }
            Text(
                when {
                    !busy -> stringResource(R.string.send_message)
                    live.isEmpty() -> "Reading your message · ${elapsedSeconds}s"
                    else -> "Writing the answer · ${elapsedSeconds}s"
                },
            )
        }
        if (busy) {
            TextButton(
                onClick = {
                    requestId += 1
                    OnDeviceEngine.cancel()
                    busy = false
                    liveAnswer.value = ""
                    canRetry = true
                    note = "Local AI request cancelled. Your conversation is still here."
                },
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.cancel_local_ai)) }
        }
        // Offered only when the last question went unanswered, and it re-sends that same
        // question rather than asking the person to retype it while they are already anxious.
        if (!busy && canRetry && messages.lastOrNull()?.fromUser == true) {
            OutlinedButton(
                onClick = ask,
                enabled = installed,
                modifier = Modifier.fillMaxWidth(),
            ) { Text(stringResource(R.string.try_again)) }
        }
    }
}

/**
 * One line of the conversation, including the one still being written.
 *
 * The live line is the same bubble as a finished one on purpose: text that appears in place and
 * then simply stops growing reads as an answer arriving, whereas a placeholder that is later
 * replaced reads as the app changing its mind.
 */
@Composable
private fun ChatBubble(
    fromUser: Boolean,
    text: String,
    label: String = if (fromUser) "YOU" else "LOCAL AI",
    muted: Boolean = false,
) {
    Box(
        modifier = Modifier.fillMaxWidth(),
        contentAlignment = if (fromUser) Alignment.CenterEnd else Alignment.CenterStart,
    ) {
        Surface(
            color = if (fromUser) ProhoriInk else ProhoriWhite,
            contentColor = if (fromUser) ProhoriWhite else ProhoriInk,
            modifier = Modifier.fillMaxWidth(0.88f),
            shape =
                if (fromUser) {
                    RoundedCornerShape(18.dp, 18.dp, 5.dp, 18.dp)
                } else {
                    RoundedCornerShape(18.dp, 18.dp, 18.dp, 5.dp)
                },
            border = if (fromUser) null else BorderStroke(1.dp, ProhoriBorder),
        ) {
            Column(Modifier.padding(14.dp)) {
                Text(
                    label,
                    style = MaterialTheme.typography.labelMedium,
                    color = if (fromUser) ProhoriGold else ProhoriGreen,
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    text,
                    color =
                        when {
                            fromUser -> ProhoriWhite
                            muted -> ProhoriMuted
                            else -> ProhoriInk
                        },
                )
            }
        }
    }
}
