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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private data class ChatLine(val fromUser: Boolean, val text: String)

@Composable
fun GeneralChatScreen() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember { ModelStore(context.applicationContext) }
    val messages = remember { mutableStateListOf<ChatLine>() }
    var installed by remember { mutableStateOf(store.installed()) }
    var input by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf<String?>(null) }

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
        Text("PRIVATE AI", style = MaterialTheme.typography.labelMedium, color = ProhoriGreen)
        Spacer(Modifier.height(6.dp))
        Text("Talk freely, stay private", style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(8.dp))
        Text(
            "Conversation stays on this phone. Chat mode never contacts hospitals or starts routing.",
            style = MaterialTheme.typography.bodyMedium,
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
                Box(
                    modifier = Modifier.fillMaxWidth(),
                    contentAlignment = if (line.fromUser) Alignment.CenterEnd else Alignment.CenterStart,
                ) {
                    Surface(
                        color = if (line.fromUser) ProhoriInk else ProhoriWhite,
                        contentColor = if (line.fromUser) ProhoriWhite else ProhoriInk,
                        modifier = Modifier.fillMaxWidth(0.88f),
                        shape =
                            if (line.fromUser) {
                                RoundedCornerShape(18.dp, 18.dp, 5.dp, 18.dp)
                            } else {
                                RoundedCornerShape(18.dp, 18.dp, 18.dp, 5.dp)
                            },
                        border = if (line.fromUser) null else BorderStroke(1.dp, ProhoriBorder),
                    ) {
                        Column(Modifier.padding(14.dp)) {
                            Text(
                                if (line.fromUser) "YOU" else "LOCAL AI",
                                style = MaterialTheme.typography.labelMedium,
                                color = if (line.fromUser) ProhoriGold else ProhoriGreen,
                            )
                            Spacer(Modifier.height(4.dp))
                            Text(line.text, color = if (line.fromUser) ProhoriWhite else ProhoriInk)
                        }
                    }
                }
            }
        }
        OutlinedTextField(
            value = input,
            onValueChange = { input = it.take(2_000) },
            label = { Text("Message local AI") },
            placeholder = { Text("Ask anything general…") },
            enabled = !busy,
            minLines = 2,
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(8.dp))
        Button(
            enabled = installed && input.isNotBlank() && !busy,
            modifier = Modifier.fillMaxWidth().height(52.dp),
            colors = ButtonDefaults.buttonColors(containerColor = ProhoriInk),
            onClick = {
                val question = input.trim()
                input = ""
                messages += ChatLine(true, question)
                busy = true
                note = "Generating locally. On this phone, the first answer can take up to one minute."
                scope.launch {
                    try {
                        val transcript =
                            messages.takeLast(8).joinToString("\n") {
                                (if (it.fromUser) "User: " else "Assistant: ") + it.text
                            }
                        val result =
                            runCatching {
                                withContext(Dispatchers.Default) {
                                    OnDeviceEngine.chat(store.modelFile, transcript)
                                }
                            }
                        result.onSuccess { run ->
                            messages += ChatLine(false, run.text)
                            val seconds = (run.metrics.totalMillis + 999) / 1_000
                            note = "Answered privately on this phone in ${seconds}s."
                        }.onFailure {
                            note = it.message ?: "The local model could not answer within one minute."
                        }
                    } finally {
                        busy = false
                    }
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
            Text(if (busy) "Thinking on device…" else "Send message")
        }
    }
}
