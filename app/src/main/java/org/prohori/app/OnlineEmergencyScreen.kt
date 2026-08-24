package org.prohori.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
fun OnlineEmergencyScreen(settings: Settings) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val locator = remember { DeviceLocation(context.applicationContext) }
    val cache = remember { OnlineRouteCache(settings) }
    var snapshot by remember { mutableStateOf<OnlineRouteSnapshot?>(null) }
    var dispatches by remember { mutableStateOf<List<HospitalDispatch>>(emptyList()) }
    var busy by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf<String?>(null) }
    var showSettings by remember { mutableStateOf(false) }
    var detailedRouteFor by remember { mutableStateOf<String?>(null) }

    fun discover() {
        val key = settings.locationIqApiKey
        if (key.isNullOrBlank()) {
            note = "Add a LocationIQ API key in Online settings first."
            showSettings = true
            return
        }
        scope.launch {
            busy = true
            dispatches = emptyList()
            detailedRouteFor = null
            note = "Getting this device's foreground location…"
            val result =
                runCatching {
                    val origin = locator.current() ?: error("No current device location was available")
                    note = "Finding hospitals and calculating all candidate ETAs in one route matrix…"
                    LocationIqClient(key).discoverRoutes(origin)
                }
            result.onSuccess {
                snapshot = it
                cache.save(it)
                note =
                    "Found ${it.routes.size} routed candidates. All registered hospitals can now be notified in parallel."
            }.onFailure { note = it.message ?: "Online hospital discovery failed." }
            busy = false
        }
    }

    val permissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
            if (grants.values.any { it }) discover() else note = "Location permission is required only for Online mode."
        }

    fun requestDiscovery() {
        val granted =
            ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_COARSE_LOCATION) ==
                PackageManager.PERMISSION_GRANTED ||
                ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) ==
                PackageManager.PERMISSION_GRANTED
        if (granted) {
            discover()
        } else {
            permissionLauncher.launch(
                arrayOf(Manifest.permission.ACCESS_COARSE_LOCATION, Manifest.permission.ACCESS_FINE_LOCATION),
            )
        }
    }

    fun notifyHospitals() {
        val current = snapshot ?: return
        val routeKey = settings.locationIqApiKey
        val transport = AlertTransports.resolve(settings)
        if (transport == null) {
            note = AlertTransports.unavailableReason(settings)
            showSettings = true
            return
        }
        scope.launch {
            busy = true
            note = "Sending registered hospital alerts in parallel through ${transport.label}…"
            val coordinator = HospitalCoordinator(transport)
            val sent = runCatching { coordinator.dispatch(current, settings.hospitalContacts()) }
            if (sent.isFailure) {
                note = sent.exceptionOrNull()?.message ?: "Hospital alerts failed."
                busy = false
                return@launch
            }
            dispatches = sent.getOrThrow()
            val awaiting = dispatches.count { it.state == HospitalAlertState.AWAITING }
            note =
                if (awaiting == 0) {
                    "No registered hospital alert was delivered. Check each contact and transport setting."
                } else {
                    "Delivered $awaiting alerts. Waiting only for explicit YES or NO replies."
                }
            busy = false
            repeat(POLL_ATTEMPTS) {
                if (dispatches.none { it.state == HospitalAlertState.AWAITING }) return@launch
                delay(POLL_INTERVAL_MILLIS)
                var updated = coordinator.poll(dispatches)
                val selected = HospitalCoordinator.bestConfirmed(updated)
                if (selected != null && detailedRouteFor != selected.route.hospital.facilityId) {
                    detailedRouteFor = selected.route.hospital.facilityId
                    note = "Hospital confirmed. Fetching its detailed route…"
                    runCatching {
                        require(!routeKey.isNullOrBlank()) { "LocationIQ API key is no longer configured" }
                        LocationIqClient(routeKey).detailedRoute(current.origin, selected.route.hospital)
                    }.onSuccess { detailed ->
                        updated =
                            updated.map { dispatch ->
                                if (dispatch.caseId == selected.caseId) dispatch.copy(route = detailed) else dispatch
                            }
                        val refreshed = (snapshot ?: current).withDetailedRoute(detailed)
                        snapshot = refreshed
                        cache.save(refreshed)
                        note = "Detailed route ready for ${detailed.hospital.name}."
                    }.onFailure {
                        note =
                            "Hospital confirmed, but detailed directions are temporarily unavailable. " +
                                "The matrix ETA and external navigation remain available."
                    }
                }
                dispatches = updated
            }
            if (dispatches.any { it.state == HospitalAlertState.AWAITING }) {
                note = "Reply polling stopped. Silence is not confirmation; call the hospitals directly."
            }
        }
    }

    val best = HospitalCoordinator.bestConfirmed(dispatches)
    LazyColumn(
        modifier = Modifier.fillMaxSize().padding(horizontal = 20.dp),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(top = 22.dp, bottom = 28.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        item {
            Text(
                "ONLINE RESPONSE",
                style = MaterialTheme.typography.labelMedium,
                color = ProhoriRed,
            )
            Spacer(Modifier.height(6.dp))
            Text("Find care that is ready", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(8.dp))
            Text(
                "Compare real road times, alert up to six hospitals together, and route only after an explicit YES.",
                style = MaterialTheme.typography.bodyMedium,
                color = ProhoriMuted,
            )
            Spacer(Modifier.height(18.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                WorkflowStep("01", "Nearby", Modifier.weight(1f))
                WorkflowStep("02", "Notify", Modifier.weight(1f))
                WorkflowStep("03", "Confirm", Modifier.weight(1f))
            }
            Spacer(Modifier.height(18.dp))
            Surface(
                color = ProhoriWhite,
                shape = RoundedCornerShape(20.dp),
                border = BorderStroke(1.dp, ProhoriBorder),
            ) {
                Column(Modifier.padding(18.dp)) {
                    Text("START HERE", style = MaterialTheme.typography.labelMedium, color = ProhoriMuted)
                    Spacer(Modifier.height(7.dp))
                    Text("Search from this phone's location", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "Your location is requested only when you tap the button.",
                        style = MaterialTheme.typography.bodySmall,
                        color = ProhoriMuted,
                    )
                    Spacer(Modifier.height(16.dp))
                    Button(
                        onClick = ::requestDiscovery,
                        enabled = !busy,
                        modifier = Modifier.fillMaxWidth().height(54.dp),
                        colors = ButtonDefaults.buttonColors(containerColor = ProhoriInk),
                    ) {
                        if (busy) {
                            CircularProgressIndicator(
                                modifier = Modifier.size(20.dp),
                                strokeWidth = 2.dp,
                                color = ProhoriWhite,
                            )
                            Spacer(Modifier.size(10.dp))
                        }
                        Text(if (busy) "Working securely…" else "Find nearby hospitals")
                    }
                    OutlinedButton(
                        onClick = { showSettings = true },
                        enabled = !busy,
                        modifier = Modifier.fillMaxWidth().height(50.dp),
                    ) { Text("Connection & hospital settings") }
                }
            }
            note?.let {
                Spacer(Modifier.height(12.dp))
                Surface(
                    color = MaterialTheme.colorScheme.secondaryContainer,
                    shape = RoundedCornerShape(12.dp),
                ) {
                    Text(it, modifier = Modifier.padding(12.dp), style = MaterialTheme.typography.bodySmall)
                }
            }
        }

        best?.let { selected ->
            item {
                Card(
                    modifier = Modifier.fillMaxWidth(),
                    colors = CardDefaults.cardColors(containerColor = ProhoriInk),
                    shape = RoundedCornerShape(22.dp),
                ) {
                    Column(Modifier.padding(20.dp)) {
                        Text("CONFIRMED • BEST ROUTE", style = MaterialTheme.typography.labelMedium, color = ProhoriGold)
                        Spacer(Modifier.height(7.dp))
                        Text(
                            selected.route.hospital.name,
                            style = MaterialTheme.typography.titleLarge,
                            color = ProhoriWhite,
                        )
                        Text(
                            "Explicit YES received. It has the shortest provider ETA among confirmed hospitals: " +
                                "about ${(selected.route.durationSeconds + 59) / 60} minutes.",
                            color = Color.White.copy(alpha = 0.76f),
                        )
                        Text(
                            if (selected.route.trafficSourceReported) {
                                "LocationIQ reported a traffic datasource for this route."
                            } else {
                                "Live traffic was not verified by the provider response."
                            },
                            color = Color.White.copy(alpha = 0.64f),
                        )
                        if (selected.route.steps.isNotEmpty()) {
                            Spacer(Modifier.height(12.dp))
                            Text("ROUTE PREVIEW", style = MaterialTheme.typography.labelMedium, color = ProhoriGold)
                            selected.route.steps.take(6).forEach {
                                Text("• ${it.instruction}", color = Color.White.copy(alpha = 0.78f))
                            }
                        }
                        Spacer(Modifier.height(14.dp))
                        Button(
                            onClick = { openNavigation(context, selected.route.hospital) },
                            colors = ButtonDefaults.buttonColors(containerColor = ProhoriGold, contentColor = ProhoriInk),
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text("Open route")
                        }
                    }
                }
            }
        }

        snapshot?.let { current ->
            item {
                Button(
                    onClick = ::notifyHospitals,
                    enabled = !busy && current.routes.isNotEmpty(),
                    modifier = Modifier.fillMaxWidth().height(54.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = ProhoriRed),
                ) { Text("Notify all registered hospitals in parallel") }
            }
            items(current.routes, key = { it.hospital.facilityId }) { route ->
                HospitalCandidateCard(
                    route = route,
                    initialChat = settings.hospitalContacts()[route.hospital.facilityId].orEmpty(),
                    dispatch = dispatches.firstOrNull { it.route.hospital.facilityId == route.hospital.facilityId },
                    onSaveChat = { value ->
                        runCatching { settings.setHospitalContact(route.hospital.facilityId, value) }
                            .onSuccess { note = "Saved a verified contact for ${route.hospital.name}." }
                            .onFailure { note = it.message }
                    },
                    onNavigate = { openNavigation(context, route.hospital) },
                )
            }
        }

        if (snapshot == null) {
            item {
                Surface(
                    color = ProhoriWhite,
                    shape = RoundedCornerShape(18.dp),
                    border = BorderStroke(1.dp, ProhoriBorder),
                ) {
                    Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Box(
                            modifier = Modifier.size(10.dp),
                            contentAlignment = Alignment.Center,
                        ) {
                            Surface(modifier = Modifier.fillMaxSize(), shape = CircleShape, color = ProhoriGreen) {}
                        }
                        Column(Modifier.padding(start = 12.dp)) {
                            Text("Silence never means acceptance", fontWeight = FontWeight.Bold)
                            Text(
                                "A hospital is selected only after a verified explicit YES.",
                                style = MaterialTheme.typography.bodySmall,
                                color = ProhoriMuted,
                            )
                        }
                    }
                }
            }
        }
    }

    if (showSettings) {
        OnlineSettingsDialog(
            settings = settings,
            onDismiss = { showSettings = false },
            onSaved = {
                showSettings = false
                note = "Online settings saved securely on this device."
            },
        )
    }
}

@Composable
private fun WorkflowStep(number: String, label: String, modifier: Modifier = Modifier) {
    Surface(
        modifier = modifier,
        color = ProhoriWhite,
        shape = RoundedCornerShape(11.dp),
        border = BorderStroke(1.dp, ProhoriBorder),
    ) {
        Column(Modifier.padding(horizontal = 10.dp, vertical = 10.dp)) {
            Text(number, color = ProhoriGold, fontSize = 11.sp, fontWeight = FontWeight.Black)
            Text(label, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun HospitalCandidateCard(
    route: OnlineHospitalRoute,
    initialChat: String,
    dispatch: HospitalDispatch?,
    onSaveChat: (String?) -> Unit,
    onNavigate: () -> Unit,
) {
    var chat by remember(route.hospital.facilityId, initialChat) { mutableStateOf(initialChat) }
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = ProhoriWhite),
        border = BorderStroke(1.dp, ProhoriBorder),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(Modifier.padding(17.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
                Text(route.hospital.name, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                Surface(color = ProhoriCanvas, shape = RoundedCornerShape(8.dp)) {
                    Text(
                        "${(route.durationSeconds + 59) / 60} MIN",
                        modifier = Modifier.padding(horizontal = 9.dp, vertical = 5.dp),
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
            }
            Text(route.hospital.displayName, style = MaterialTheme.typography.bodyMedium)
            Text(
                "${route.hospital.kind} · ${route.distanceMetres / 1_000.0} km by route · " +
                    "about ${(route.durationSeconds + 59) / 60} min",
            )
            Text(
                if (route.trafficSourceReported) "Provider traffic datasource reported"
                else "Traffic not verified",
                style = MaterialTheme.typography.bodyMedium,
            )
            OutlinedTextField(
                value = chat,
                onValueChange = { chat = it.take(33) },
                label = { Text("Verified Telegram chat id") },
                supportingText = { Text("Hospital staff must start/add the bot first.") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(onClick = { onSaveChat(chat.trim().ifBlank { null }) }) { Text("Save contact") }
                TextButton(onClick = onNavigate) { Text("Open route") }
            }
            dispatch?.let {
                HorizontalDivider()
                Surface(
                    color = if (it.state == HospitalAlertState.CONFIRMED) ProhoriGreenSoft else ProhoriCanvas,
                    shape = RoundedCornerShape(9.dp),
                ) {
                    Text(
                        "${it.state.name.lowercase().replace('_', ' ')} · ${it.detail}",
                        modifier = Modifier.padding(9.dp),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
            if (route.steps.isNotEmpty()) {
                Text("Route preview", style = MaterialTheme.typography.titleMedium)
                route.steps.take(4).forEach { Text("• ${it.instruction}") }
            }
        }
    }
}

@Composable
private fun OnlineSettingsDialog(settings: Settings, onDismiss: () -> Unit, onSaved: () -> Unit) {
    var locationIq by remember { mutableStateOf(settings.locationIqApiKey.orEmpty()) }
    var relayUrl by remember { mutableStateOf(settings.relayBaseUrl.orEmpty()) }
    var relayToken by remember { mutableStateOf(settings.relayDeviceToken.orEmpty()) }
    var botToken by remember { mutableStateOf(settings.telegramBotToken.orEmpty()) }
    var error by remember { mutableStateOf<String?>(null) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Online settings") },
        text = {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                item {
                    SecretField("LocationIQ API key", locationIq) { locationIq = it }
                    OutlinedTextField(
                        value = relayUrl,
                        onValueChange = { relayUrl = it.take(240) },
                        label = { Text("Relay HTTPS URL (preferred)") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    SecretField("Relay device token", relayToken) { relayToken = it }
                    SecretField("Personal Telegram bot token", botToken) { botToken = it }
                    Text(
                        "Use either relay credentials or a bot dedicated to this phone. A bot shared by several phones can lose replies.",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    val normalizedRelay = relayUrl.trim().removeSuffix("/")
                    when {
                        normalizedRelay.isNotEmpty() && !isAcceptableRelayBaseUrl(normalizedRelay) ->
                            error = "Relay URL must use HTTPS (or debug loopback)."
                        botToken.isNotBlank() && !Regex("^[0-9]{5,15}:[A-Za-z0-9_-]{20,}$").matches(botToken.trim()) ->
                            error = "Personal Telegram bot token format is not valid."
                        else -> {
                            settings.locationIqApiKey = locationIq
                            settings.relayBaseUrl = normalizedRelay
                            settings.relayDeviceToken = relayToken
                            settings.telegramBotToken = botToken
                            onSaved()
                        }
                    }
                },
            ) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun SecretField(label: String, value: String, onChange: (String) -> Unit) {
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

private fun openNavigation(context: android.content.Context, hospital: OnlineHospital) {
    val destination = "${hospital.location.latitude},${hospital.location.longitude}"
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse("geo:0,0?q=$destination(${Uri.encode(hospital.name)})"))
    runCatching { context.startActivity(intent) }
}

private const val POLL_ATTEMPTS = 24
private const val POLL_INTERVAL_MILLIS = 5_000L
