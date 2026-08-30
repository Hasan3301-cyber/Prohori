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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.platform.testTag
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
import java.text.DateFormat
import java.util.Date

@Composable
fun OnlineEmergencyScreen(
    settings: Settings,
    requestedSpecialty: String = "general_emergency",
    onOpenSettings: () -> Unit = {},
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val locator = remember { DeviceLocation(context.applicationContext) }
    val cache = remember { OnlineRouteCache(settings) }
    var snapshot by remember { mutableStateOf<OnlineRouteSnapshot?>(null) }
    var dispatches by remember { mutableStateOf<List<HospitalDispatch>>(emptyList()) }
    var busy by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf<String?>(null) }
    var detailedRouteFor by remember { mutableStateOf<String?>(null) }
    var phase by remember { mutableStateOf(OnlinePhase.READY) }

    fun discover() {
        val key = settings.locationIqApiKey
        if (key.isNullOrBlank()) {
            note = "Add a LocationIQ API key in Online settings first."
            phase = OnlinePhase.ERROR
            onOpenSettings()
            return
        }
        scope.launch {
            busy = true
            dispatches = emptyList()
            detailedRouteFor = null
            note = "Getting this device's foreground location…"
            phase = OnlinePhase.LOCATING
            val result =
                runCatching {
                    val origin = locator.current() ?: error("No current device location was available")
                    phase = OnlinePhase.DISCOVERING
                    note = "Finding hospitals and calculating all candidate ETAs in one route matrix…"
                    LocationIqClient(key).discoverRoutes(origin)
                }
            result.onSuccess {
                snapshot = it
                cache.save(it)
                note =
                    "Found ${it.routes.size} routed candidates. All registered hospitals can now be notified in parallel."
                phase = OnlinePhase.ROUTES_READY
            }.onFailure {
                note = onlineFailureMessage(it)
                phase = OnlinePhase.ERROR
            }
            busy = false
        }
    }

    val permissionLauncher =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
            if (grants.values.any { it }) {
                discover()
            } else {
                note = "Location permission was denied. Allow it in Android Settings, or use the cached route in Offline mode."
                phase = OnlinePhase.ERROR
            }
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
            phase = OnlinePhase.ERROR
            onOpenSettings()
            return
        }
        scope.launch {
            busy = true
            phase = OnlinePhase.NOTIFYING
            note = "Sending registered hospital alerts in parallel through ${transport.label}…"
            val coordinator = HospitalCoordinator(transport)
            val sent =
                runCatching {
                    coordinator.dispatch(current, settings.hospitalContacts(), requestedSpecialty)
                }
            if (sent.isFailure) {
                note = sent.exceptionOrNull()?.message ?: "Hospital alerts failed."
                phase = OnlinePhase.ERROR
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
            phase = if (awaiting > 0) OnlinePhase.WAITING else OnlinePhase.ERROR
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
                        phase = OnlinePhase.CONFIRMED
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

    fun retryHospital(route: OnlineHospitalRoute) {
        val current = snapshot ?: return
        val transport = AlertTransports.resolve(settings)
        if (transport == null) {
            note = AlertTransports.unavailableReason(settings)
            onOpenSettings()
            return
        }
        scope.launch {
            busy = true
            phase = OnlinePhase.NOTIFYING
            note = "Retrying ${route.hospital.name} only…"
            val coordinator = HospitalCoordinator(transport)
            val retried =
                runCatching {
                    coordinator.dispatch(
                        current.copy(routes = listOf(route)),
                        settings.hospitalContacts(),
                        requestedSpecialty,
                    ).single()
                }.getOrElse {
                    note = it.message ?: "This hospital could not be contacted."
                    phase = OnlinePhase.ERROR
                    busy = false
                    return@launch
                }
            dispatches = dispatches.filterNot { it.route.hospital.facilityId == route.hospital.facilityId } + retried
            busy = false
            if (retried.state != HospitalAlertState.AWAITING) {
                phase = OnlinePhase.ERROR
                note = retried.detail
                return@launch
            }
            phase = OnlinePhase.WAITING
            note = "Alert delivered again. Waiting only for an explicit YES or NO."
            repeat(POLL_ATTEMPTS) {
                delay(POLL_INTERVAL_MILLIS)
                val updated = coordinator.poll(listOf(retried)).single()
                dispatches = dispatches.map { if (it.caseId == retried.caseId) updated else it }
                if (updated.state != HospitalAlertState.AWAITING) {
                    phase = if (updated.state == HospitalAlertState.CONFIRMED) OnlinePhase.CONFIRMED else OnlinePhase.ROUTES_READY
                    note = updated.detail
                    return@launch
                }
            }
            note = "No reply was received. Silence is not confirmation; call this hospital directly."
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
                stringResource(R.string.online_section),
                style = MaterialTheme.typography.labelMedium,
                color = ProhoriRed,
            )
            Spacer(Modifier.height(6.dp))
            Text(stringResource(R.string.online_title), style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(8.dp))
            Text(
                stringResource(R.string.online_body),
                style = MaterialTheme.typography.bodyMedium,
                color = ProhoriMuted,
            )
            Spacer(Modifier.height(8.dp))
            Surface(color = ProhoriGreenSoft, shape = RoundedCornerShape(9.dp)) {
                Text(
                    "Hospital service requested: ${specialtyDisplayName(requestedSpecialty)}. " +
                        "This category is not a diagnosis and contains no symptom text.",
                    modifier = Modifier.padding(horizontal = 11.dp, vertical = 8.dp),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Spacer(Modifier.height(18.dp))
            OnlineTimeline(phase = phase, snapshot = snapshot, dispatches = dispatches)
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
                        modifier = Modifier.fillMaxWidth().height(54.dp).testTag("online_find_hospitals"),
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
                        Text(if (busy) stringResource(R.string.working_securely) else stringResource(R.string.find_nearby_hospitals))
                    }
                    OutlinedButton(
                        onClick = onOpenSettings,
                        enabled = !busy,
                        modifier = Modifier.fillMaxWidth().height(50.dp),
                    ) { Text(stringResource(R.string.connection_settings)) }
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
                    modifier = Modifier.fillMaxWidth().height(54.dp).testTag("online_notify_parallel"),
                    colors = ButtonDefaults.buttonColors(containerColor = ProhoriRed),
                ) { Text(stringResource(R.string.notify_parallel)) }
            }
            items(current.routes, key = { it.hospital.facilityId }) { route ->
                HospitalCandidateCard(
                    route = route,
                    initialContact =
                        settings.hospitalEndpoints()[route.hospital.facilityId] ?: HospitalContact(),
                    dispatch = dispatches.firstOrNull { it.route.hospital.facilityId == route.hospital.facilityId },
                    fetchedAtEpochMillis = current.fetchedAtEpochMillis,
                    onSaveContact = { value ->
                        runCatching { settings.setHospitalContact(route.hospital.facilityId, value) }
                            .onSuccess { note = "Saved verified contact options for ${route.hospital.name}." }
                            .onFailure { note = it.message }
                    },
                    onCall = { number ->
                        if (!dial(context, number)) note = "No dialer opened. Dial $number by hand."
                    },
                    onSms = { number ->
                        val body =
                            hospitalReadinessSms(
                                route.hospital.name,
                                requestedSpecialty,
                                (route.durationSeconds + 59) / 60,
                            )
                        if (!composeHospitalSms(context, number, body)) note = "No SMS app opened."
                    },
                    onNavigate = { openNavigation(context, route.hospital) },
                    onRetry = { retryHospital(route) },
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

}

private enum class OnlinePhase { READY, LOCATING, DISCOVERING, ROUTES_READY, NOTIFYING, WAITING, CONFIRMED, ERROR }

@Composable
private fun OnlineTimeline(
    phase: OnlinePhase,
    snapshot: OnlineRouteSnapshot?,
    dispatches: List<HospitalDispatch>,
) {
    val discovered = snapshot != null
    val contacted = dispatches.any { it.sentAtEpochMillis != null }
    val confirmed = dispatches.any { it.state == HospitalAlertState.CONFIRMED }
    Column(verticalArrangement = Arrangement.spacedBy(7.dp)) {
        WorkflowStep("01", if (discovered) "Nearby hospitals found · ${formatClock(snapshot!!.fetchedAtEpochMillis)}" else "Find nearby hospitals", discovered || phase == OnlinePhase.LOCATING || phase == OnlinePhase.DISCOVERING)
        WorkflowStep("02", if (contacted) "Alerts sent to ${dispatches.count { it.sentAtEpochMillis != null }} hospitals" else "Notify up to six together", contacted || phase == OnlinePhase.NOTIFYING)
        WorkflowStep("03", if (confirmed) "Explicit YES received" else "Wait for explicit YES or NO", confirmed || phase == OnlinePhase.WAITING)
    }
}

@Composable
private fun WorkflowStep(number: String, label: String, active: Boolean, modifier: Modifier = Modifier) {
    Surface(
        modifier = modifier,
        color = if (active) ProhoriGreenSoft else ProhoriWhite,
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
    initialContact: HospitalContact,
    dispatch: HospitalDispatch?,
    fetchedAtEpochMillis: Long,
    onSaveContact: (HospitalContact?) -> Unit,
    onCall: (String) -> Unit,
    onSms: (String) -> Unit,
    onNavigate: () -> Unit,
    onRetry: () -> Unit,
) {
    var chat by remember(route.hospital.facilityId, initialContact) {
        mutableStateOf(initialContact.telegramChatId.orEmpty())
    }
    var hotline by remember(route.hospital.facilityId, initialContact) {
        mutableStateOf(initialContact.hotline.orEmpty())
    }
    var sms by remember(route.hospital.facilityId, initialContact) {
        mutableStateOf(initialContact.smsNumber.orEmpty())
    }
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
            Text(
                "Route measured ${formatClock(fetchedAtEpochMillis)} · " +
                    cachedRouteAgeLabel(fetchedAtEpochMillis, System.currentTimeMillis()) +
                    if (routeIsStale(fetchedAtEpochMillis)) " · refresh recommended" else "",
                style = MaterialTheme.typography.bodySmall,
                color = if (routeIsStale(fetchedAtEpochMillis)) ProhoriRed else ProhoriMuted,
            )
            OutlinedTextField(
                value = chat,
                onValueChange = { chat = it.take(33) },
                label = { Text("Verified Telegram chat id") },
                supportingText = { Text("Hospital staff must start/add the bot first.") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = hotline,
                onValueChange = { hotline = it.take(26) },
                label = { Text("Verified hotline") },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Phone),
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = sms,
                onValueChange = { sms = it.take(26) },
                label = { Text("Verified SMS number") },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Phone),
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(
                    onClick = {
                        onSaveContact(
                            HospitalContact(
                                telegramChatId = chat.trim().ifBlank { null },
                                hotline = hotline.trim().ifBlank { null },
                                smsNumber = sms.trim().ifBlank { null },
                            ),
                        )
                    },
                ) { Text("Save contacts") }
                TextButton(onClick = onNavigate) { Text("Open route") }
            }
            if (hotline.isNotBlank() || sms.isNotBlank()) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (hotline.isNotBlank()) {
                        OutlinedButton(onClick = { onCall(hotline.trim()) }) { Text("Call") }
                    }
                    if (sms.isNotBlank()) {
                        OutlinedButton(onClick = { onSms(sms.trim()) }) { Text("Prepare SMS") }
                    }
                }
                Text(
                    "Android always leaves the final call or Send action to you.",
                    style = MaterialTheme.typography.bodySmall,
                    color = ProhoriMuted,
                )
            }
            dispatch?.let {
                HorizontalDivider()
                Surface(
                    color = if (it.state == HospitalAlertState.CONFIRMED) ProhoriGreenSoft else ProhoriCanvas,
                    shape = RoundedCornerShape(9.dp),
                ) {
                    Text(
                        hospitalStatusLabel(it) + " · " + it.detail +
                            (it.repliedAtEpochMillis ?: it.sentAtEpochMillis)?.let { time -> " · ${formatClock(time)}" }.orEmpty(),
                        modifier = Modifier.padding(9.dp),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                if (it.state == HospitalAlertState.FAILED || it.state == HospitalAlertState.DECLINED) {
                    OutlinedButton(onClick = onRetry, modifier = Modifier.fillMaxWidth()) { Text("Retry this hospital") }
                }
            }
            if (route.steps.isNotEmpty()) {
                Text("Route preview", style = MaterialTheme.typography.titleMedium)
                route.steps.take(4).forEach { Text("• ${it.instruction}") }
            }
        }
    }
}

internal fun routeIsStale(fetchedAtEpochMillis: Long, nowMillis: Long = System.currentTimeMillis()): Boolean =
    nowMillis - fetchedAtEpochMillis > 15 * 60_000L

private fun hospitalStatusLabel(dispatch: HospitalDispatch): String =
    when (dispatch.state) {
        HospitalAlertState.UNREGISTERED -> "Contact not registered"
        HospitalAlertState.SENDING -> "Sending"
        HospitalAlertState.AWAITING -> "Delivered; awaiting reply"
        HospitalAlertState.CONFIRMED -> "Explicit YES"
        HospitalAlertState.DECLINED -> "Explicit NO"
        HospitalAlertState.FAILED -> "Delivery failed"
    }

private fun formatClock(epochMillis: Long): String = DateFormat.getTimeInstance(DateFormat.SHORT).format(Date(epochMillis))

private fun onlineFailureMessage(error: Throwable): String {
    val raw = error.message.orEmpty()
    return when {
        raw.contains("429") -> "Location service rate limit reached. Wait briefly, then try again; Offline mode still works."
        raw.contains("401") || raw.contains("403") -> "LocationIQ rejected the API key. Check it in Settings and try again."
        raw.contains("location", ignoreCase = true) -> "Current location was unavailable. Turn on Location and try again, or use the cached route in Offline mode."
        raw.contains("network", ignoreCase = true) || raw.contains("connect", ignoreCase = true) -> "No working internet connection. Use Offline mode now, then retry when signal returns."
        else -> raw.ifBlank { "Online hospital discovery failed. Check internet, location, and the LocationIQ key, then retry." }
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
