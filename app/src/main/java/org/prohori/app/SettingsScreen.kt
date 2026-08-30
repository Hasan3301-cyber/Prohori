package org.prohori.app

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.prohori.core.CityPackInstall
import org.prohori.core.CountryChoice
import org.prohori.core.Prohori

/** Operator configuration, deliberately outside every emergency decision path. */
@Composable
fun AppSettingsScreen(core: Prohori, settings: Settings, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val modelStore = remember { ModelStore(context.applicationContext) }
    val cityPackStore = remember { CityPackStore(context.applicationContext) }
    var selectedCountry by remember { mutableStateOf(settings.country ?: CountryHint.detect(context)) }
    var ambulance by remember { mutableStateOf(settings.ambulanceOverride.orEmpty()) }
    var locationIq by remember { mutableStateOf(settings.locationIqApiKey.orEmpty()) }
    var relayUrl by remember { mutableStateOf(settings.relayBaseUrl.orEmpty()) }
    var relayToken by remember { mutableStateOf(settings.relayDeviceToken.orEmpty()) }
    var botToken by remember { mutableStateOf(settings.telegramBotToken.orEmpty()) }
    var showCountries by remember { mutableStateOf(false) }
    var status by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var modelReady by remember { mutableStateOf(modelStore.installed()) }
    var cityPack by remember { mutableStateOf(runCatching { cityPackStore.installActiveOrBundled(core) }.getOrNull()) }

    val modelPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri == null) return@rememberLauncherForActivityResult
            scope.launch {
                busy = true
                status = "Checking and copying the selected private model…"
                val result = runCatching { withContext(Dispatchers.IO) { modelStore.import(uri) } }
                result.onSuccess {
                    modelReady = true
                    status = "Private model ready · ${it.bytes / 1_000_000} MB"
                }.onFailure { status = "Model was not changed: ${it.message ?: "unknown error"}" }
                busy = false
            }
        }
    val cityPackPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri == null) return@rememberLauncherForActivityResult
            scope.launch {
                busy = true
                status = "Checking the signed offline route data…"
                val result = runCatching { withContext(Dispatchers.IO) { cityPackStore.import(uri, core) } }
                result.onSuccess {
                    cityPack = it
                    status = if (it.accepted) "Offline route data updated." else "Route data refused: ${it.error}"
                }.onFailure { status = "Route data was not changed: ${it.message ?: "unknown error"}" }
                busy = false
            }
        }

    Column(
        modifier = Modifier.verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 18.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text("Settings", style = MaterialTheme.typography.titleLarge)
        Text(
            "These controls configure the app. They are kept away from emergency guidance so the main screen stays simple.",
            color = ProhoriMuted,
        )

        SettingsSection("Emergency call") {
            Text("Country: ${selectedCountry ?: "automatic"}", style = MaterialTheme.typography.bodyLarge)
            OutlinedButton(onClick = { showCountries = true }, modifier = Modifier.fillMaxWidth()) {
                Text("Choose country")
            }
            OutlinedTextField(
                value = ambulance,
                onValueChange = { ambulance = it.take(32) },
                label = { Text("Local ambulance number override") },
                supportingText = { Text("Leave empty to use the selected country's number.") },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Phone),
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
        }

        SettingsSection("Online services") {
            SecretSetting("LocationIQ API key", locationIq) { locationIq = it }
            OutlinedTextField(
                value = relayUrl,
                onValueChange = { relayUrl = it.take(240) },
                label = { Text("Relay HTTPS URL (preferred)") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            SecretSetting("Relay device token", relayToken) { relayToken = it }
            SecretSetting("Personal Telegram bot token", botToken) { botToken = it }
            Text(
                "Use a relay or a bot dedicated to this phone. Message delivery is never treated as hospital confirmation.",
                style = MaterialTheme.typography.bodySmall,
                color = ProhoriMuted,
            )
        }

        SettingsSection("Private offline AI") {
            Text(
                if (modelReady) "Bundled model ready on this device." else "No usable local model is installed.",
                style = MaterialTheme.typography.bodyLarge,
            )
            Text(
                "The bundled model is about 1.1 GB. Replacing it is an advanced recovery action; ordinary users never need this.",
                style = MaterialTheme.typography.bodySmall,
                color = ProhoriMuted,
            )
            OutlinedButton(
                onClick = { modelPicker.launch(arrayOf("application/octet-stream", "*/*")) },
                enabled = !busy,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Replace private model file") }
        }

        SettingsSection("Offline route data") {
            CityPackStatus(cityPack)
            OutlinedButton(
                onClick = { cityPackPicker.launch(arrayOf("application/zip", "application/octet-stream", "*/*")) },
                enabled = !busy,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Import signed route-data update") }
        }

        SettingsSection("Privacy") {
            Text("Symptoms and chat stay on this phone.")
            Text("Hospital alerts contain case id, facility, specialty, and ETA—not symptoms, name, or coordinates.")
            Text("API keys and contact identifiers are encrypted in this device's private storage.")
        }

        status?.let { Text(it, color = if (it.contains("refused", true) || it.contains("not changed", true)) ProhoriRed else ProhoriGreen) }
        Button(
            onClick = {
                val normalizedRelay = relayUrl.trim().removeSuffix("/")
                when {
                    normalizedRelay.isNotEmpty() && !isAcceptableRelayBaseUrl(normalizedRelay) ->
                        status = "Relay URL must use HTTPS, except for debug loopback."
                    botToken.isNotBlank() && !Regex("^[0-9]{5,15}:[A-Za-z0-9_-]{20,}$").matches(botToken.trim()) ->
                        status = "The Telegram bot token format is not valid."
                    else -> {
                        settings.country = selectedCountry
                        settings.ambulanceOverride = ambulance.trim().ifBlank { null }
                        settings.locationIqApiKey = locationIq
                        settings.relayBaseUrl = normalizedRelay
                        settings.relayDeviceToken = relayToken
                        settings.telegramBotToken = botToken
                        status = "Settings saved securely on this phone."
                    }
                }
            },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth().height(54.dp),
        ) { Text(if (busy) "Working…" else "Save settings") }
        TextButton(onClick = onBack, modifier = Modifier.fillMaxWidth()) { Text("Back to Prohori") }
        Spacer(Modifier.height(20.dp))
    }

    if (showCountries) {
        CountryChooser(
            countries = remember { core.knownCountries() },
            selected = selectedCountry,
            onSelect = {
                selectedCountry = it
                showCountries = false
            },
            onDismiss = { showCountries = false },
        )
    }
}

@Composable
private fun SettingsSection(title: String, content: @Composable () -> Unit) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(18.dp),
        color = ProhoriWhite,
        border = BorderStroke(1.dp, ProhoriBorder),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            content()
        }
    }
}

@Composable
private fun SecretSetting(label: String, value: String, onChange: (String) -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = { onChange(it.take(300)) },
        label = { Text(label) },
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
        singleLine = true,
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun CityPackStatus(install: CityPackInstall?) {
    when {
        install == null -> Text("No offline route data is available.", color = ProhoriRed)
        !install.accepted -> Text("Offline route data was refused: ${install.error}", color = ProhoriRed)
        else -> {
            Text("${install.city ?: "City data"} · version ${install.version ?: 0u}")
            Text(
                if (install.fieldChecked) "Marked as field checked." else "Demo data—not field checked for real navigation.",
                color = if (install.fieldChecked) ProhoriGreen else ProhoriRed,
                fontWeight = FontWeight.Bold,
            )
        }
    }
}

@Composable
private fun CountryChooser(
    countries: List<CountryChoice>,
    selected: String?,
    onSelect: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Choose country") },
        text = {
            LazyColumn {
                items(countries) { choice ->
                    TextButton(onClick = { onSelect(choice.code) }, modifier = Modifier.fillMaxWidth()) {
                        Text(
                            (if (choice.code == selected) "✓ " else "") + "${choice.name} · ${choice.ambulance}",
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
