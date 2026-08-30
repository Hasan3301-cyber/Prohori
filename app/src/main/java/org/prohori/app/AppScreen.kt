package org.prohori.app

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.withContext
import org.prohori.core.Prohori

@Composable
fun AppScreen(core: Prohori, settings: Settings) {
    var onboardingSeen by remember { mutableStateOf(settings.onboardingSeen) }
    if (!onboardingSeen) {
        ProhoriOnboarding(
            onContinue = {
                settings.onboardingSeen = true
                onboardingSeen = true
            },
        )
        return
    }
    val context = LocalContext.current
    val modelStore = remember { ModelStore(context.applicationContext) }
    var attempt by remember { mutableIntStateOf(0) }
    var modelState by remember {
        mutableStateOf<BundledModelState>(
            if (modelStore.installed()) BundledModelState.Ready else BundledModelState.Preparing,
        )
    }
    // Written from the copy thread, read by the screen, so it travels as a flow rather than
    // as snapshot state.
    val copiedBytes = remember { MutableStateFlow(0L) }

    LaunchedEffect(attempt) {
        if (modelState == BundledModelState.Ready || modelState == BundledModelState.Skipped) {
            return@LaunchedEffect
        }
        modelState = BundledModelState.Preparing
        copiedBytes.value = 0L
        val result =
            withContext(Dispatchers.IO) {
                runCatching { modelStore.installBundled { copied -> copiedBytes.value = copied } }
            }
        modelState =
            result.fold(
                onSuccess = { BundledModelState.Ready },
                onFailure = {
                    BundledModelState.Failed(
                        it.message ?: "The bundled local AI model could not be prepared.",
                    )
                },
            )
    }

    when (val state = modelState) {
        BundledModelState.Ready,
        BundledModelState.Skipped -> AppModes(core, settings)
        BundledModelState.Preparing -> BundledModelPreparation(copiedBytes)
        is BundledModelState.Failed ->
            BundledModelFailure(
                message = state.message,
                onRetry = { attempt += 1 },
                onContinue = { modelState = BundledModelState.Skipped },
            )
    }
}

@Composable
private fun AppModes(core: Prohori, settings: Settings) {
    var mode by remember { mutableStateOf(settings.appMode) }
    var showSettings by remember { mutableStateOf(false) }
    // Only a coarse service category crosses modes. Patient wording is never persisted.
    var requestedSpecialty by remember { mutableStateOf("general_emergency") }
    Scaffold(
        containerColor = ProhoriCanvas,
        topBar = {
            if (showSettings) {
                SettingsHeader(onBack = { showSettings = false })
            } else {
                ProhoriHeader(mode, onSettings = { showSettings = true })
            }
        },
        bottomBar = {
            if (!showSettings) {
                Surface(
                    color = ProhoriPaper,
                    shadowElevation = 12.dp,
                    border = BorderStroke(1.dp, ProhoriBorder),
                ) {
                    NavigationBar(
                        containerColor = ProhoriPaper,
                        tonalElevation = 0.dp,
                        modifier = Modifier.height(84.dp),
                    ) {
                        AppMode.entries.forEach { choice ->
                            NavigationBarItem(
                                selected = mode == choice,
                                onClick = {
                                    mode = choice
                                    settings.appMode = choice
                                },
                                icon = {
                                    Text(
                                        choice.shortLabel,
                                        fontSize = 12.sp,
                                        fontWeight = FontWeight.Black,
                                        letterSpacing = 0.7.sp,
                                    )
                                },
                                label = { Text(stringResource(choice.labelRes), fontWeight = FontWeight.SemiBold) },
                                colors =
                                    NavigationBarItemDefaults.colors(
                                        selectedIconColor = ProhoriWhite,
                                        selectedTextColor = ProhoriInk,
                                        indicatorColor = ProhoriInk,
                                        unselectedIconColor = ProhoriMuted,
                                        unselectedTextColor = ProhoriMuted,
                                    ),
                                )
                        }
                    }
                }
            }
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            if (showSettings) {
                AppSettingsScreen(core, settings, onBack = { showSettings = false })
            } else {
                when (mode) {
                    AppMode.OFFLINE -> OfflineMode(core, settings, mode)
                    AppMode.ONLINE ->
                        OnlineEmergencyScreen(
                            settings,
                            requestedSpecialty = requestedSpecialty,
                            onOpenSettings = { showSettings = true },
                        )
                    AppMode.CHAT ->
                        GeneralChatScreen(
                            core = core,
                            onOpenEmergency = {
                                mode = AppMode.OFFLINE
                                settings.appMode = AppMode.OFFLINE
                            },
                            onFindHospitals = { specialty ->
                                requestedSpecialty = specialty
                                mode = AppMode.ONLINE
                                settings.appMode = AppMode.ONLINE
                            },
                        )
                }
            }
        }
    }
}

@Composable
private fun ProhoriHeader(mode: AppMode, onSettings: () -> Unit) {
    Surface(color = ProhoriInk) {
        Row(
            modifier =
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 88.dp)
                    .padding(horizontal = 20.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ProhoriBrandMark()
            Column(Modifier.padding(start = 13.dp).weight(1f)) {
                Text(
                    "PROHORI",
                    color = ProhoriWhite,
                    fontSize = 18.sp,
                    fontWeight = FontWeight.Black,
                    letterSpacing = 1.5.sp,
                )
                Text(stringResource(mode.subtitleRes), color = Color.White.copy(alpha = 0.66f), fontSize = 13.sp)
            }
            Surface(
                color = Color.White.copy(alpha = 0.1f),
                shape = RoundedCornerShape(8.dp),
                border = BorderStroke(1.dp, Color.White.copy(alpha = 0.14f)),
            ) {
                Text(
                    stringResource(mode.statusRes),
                    modifier = Modifier.padding(horizontal = 9.dp, vertical = 6.dp),
                    color = ProhoriWhite,
                    fontSize = 10.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.8.sp,
                )
            }
            TextButton(
                onClick = onSettings,
                colors = ButtonDefaults.textButtonColors(contentColor = ProhoriWhite),
            ) {
                Text(stringResource(R.string.settings), fontSize = 10.sp, fontWeight = FontWeight.Bold, letterSpacing = 0.6.sp)
            }
        }
    }
}

@Composable
private fun SettingsHeader(onBack: () -> Unit) {
    Surface(color = ProhoriInk) {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 76.dp).padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TextButton(
                onClick = onBack,
                colors = ButtonDefaults.textButtonColors(contentColor = ProhoriWhite),
            ) { Text(stringResource(R.string.back)) }
            Column(Modifier.padding(start = 8.dp)) {
                Text(stringResource(R.string.settings), color = ProhoriWhite, fontWeight = FontWeight.Black, letterSpacing = 1.2.sp)
                Text(stringResource(R.string.settings_subtitle), color = Color.White.copy(alpha = 0.66f), fontSize = 13.sp)
            }
        }
    }
}

@Composable
private fun ProhoriOnboarding(onContinue: () -> Unit) {
    FirstLaunchFrame(
        title = stringResource(R.string.onboarding_title),
        body = stringResource(R.string.onboarding_body),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(stringResource(R.string.onboarding_model))
            Text(stringResource(R.string.onboarding_storage))
            Text("• Preparing it copies and verifies a gigabyte, so the next screen takes a few minutes. It happens once.")
            Text("• Offline mode needs no keys or account. Keys in Settings are optional and only add hospital alerts and live routes.")
            Text(stringResource(R.string.onboarding_confirmation))
            Text(stringResource(R.string.onboarding_disclaimer))
            Spacer(Modifier.height(12.dp))
            Button(
                onClick = onContinue,
                colors = ButtonDefaults.buttonColors(containerColor = ProhoriInk),
                modifier = Modifier.fillMaxWidth().height(54.dp),
            ) { Text(stringResource(R.string.onboarding_continue)) }
        }
    }
}

@Composable
private fun ProhoriBrandMark(modifier: Modifier = Modifier) {
    Box(
        modifier =
            modifier
                .size(48.dp)
                .clip(RoundedCornerShape(15.dp))
                .background(ProhoriRed),
        contentAlignment = Alignment.Center,
    ) {
        Image(
            painter = painterResource(R.drawable.ic_launcher_foreground),
            contentDescription = null,
            modifier = Modifier.size(48.dp),
        )
    }
}

@Composable
private fun BundledModelPreparation(copiedBytes: MutableStateFlow<Long>) {
    val copied by copiedBytes.collectAsState()
    var elapsedSeconds by remember { mutableIntStateOf(0) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1_000)
            elapsedSeconds += 1
        }
    }
    FirstLaunchFrame(
        title = "Preparing your private AI",
        body =
            "This happens once. The verified model is included in the app, and Prohori is " +
                "copying it into private storage and checking every byte before first use. " +
                "It is a gigabyte, so it takes a few minutes on most phones.",
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            LinearProgressIndicator(
                progress = {
                    (copied.toFloat() / BUNDLED_MODEL_BYTES.toFloat()).coerceIn(0f, 1f)
                },
                color = ProhoriGreen,
                trackColor = ProhoriBorder,
                modifier = Modifier.fillMaxWidth().height(8.dp).testTag("model_prepare_progress"),
            )
            Spacer(Modifier.height(14.dp))
            Text(
                preparationProgressLabel(copied, BUNDLED_MODEL_BYTES, elapsedSeconds),
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.testTag("model_prepare_label"),
            )
            Spacer(Modifier.height(18.dp))
            Text("KEEP THE APP OPEN", style = MaterialTheme.typography.labelMedium, color = ProhoriMuted)
        }
    }
}

@Composable
private fun FirstLaunchFrame(
    title: String,
    body: String,
    content: @Composable () -> Unit,
) {
    Column(
        modifier = Modifier.fillMaxSize().background(ProhoriInk),
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().height(210.dp).padding(horizontal = 28.dp),
            verticalArrangement = Arrangement.Center,
        ) {
            ProhoriBrandMark()
            Spacer(Modifier.height(16.dp))
            Text("PROHORI", color = ProhoriWhite, fontWeight = FontWeight.Black, letterSpacing = 2.sp)
            Text("Private emergency intelligence", color = Color.White.copy(alpha = 0.62f))
        }
        Surface(
            modifier = Modifier.fillMaxSize(),
            color = ProhoriPaper,
            shape = RoundedCornerShape(topStart = 34.dp, topEnd = 34.dp),
        ) {
            // Scrollable because the onboarding list grew and the button underneath it is the
            // only way forward. A first-launch screen whose primary action is off the bottom
            // edge of a small phone is an app that cannot be opened at all.
            Column(
                modifier =
                    Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(horizontal = 28.dp, vertical = 40.dp),
                horizontalAlignment = Alignment.Start,
            ) {
                Text(title, style = MaterialTheme.typography.titleLarge)
                Spacer(Modifier.height(12.dp))
                Text(body, style = MaterialTheme.typography.bodyMedium, color = ProhoriMuted)
                Spacer(Modifier.height(34.dp))
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { content() }
            }
        }
    }
}

@Composable
private fun BundledModelFailure(message: String, onRetry: () -> Unit, onContinue: () -> Unit) {
    FirstLaunchFrame(
        title = "Local AI needs attention",
        body = message,
    ) {
        Button(
            onClick = onRetry,
            colors = ButtonDefaults.buttonColors(containerColor = ProhoriInk),
            modifier = Modifier.fillMaxWidth().height(54.dp),
        ) { Text("Try again") }
        TextButton(onClick = onContinue, modifier = Modifier.fillMaxWidth()) {
            Text("Continue without local AI")
        }
    }
}

private sealed interface BundledModelState {
    data object Preparing : BundledModelState
    data object Ready : BundledModelState
    data object Skipped : BundledModelState
    data class Failed(val message: String) : BundledModelState
}

@Composable
private fun OfflineMode(core: Prohori, settings: Settings, refreshKey: AppMode) {
    val context = LocalContext.current
    val cached = remember(refreshKey) { OnlineRouteCache(settings).load() }
    Column(Modifier.fillMaxSize()) {
        cached?.routes?.firstOrNull()?.let { route ->
            val contact = settings.hospitalEndpoints()[route.hospital.facilityId]
            Surface(
                color = ProhoriGreenSoft,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                shape = RoundedCornerShape(14.dp),
                border = BorderStroke(1.dp, ProhoriGreen.copy(alpha = 0.22f)),
            ) {
                Column(Modifier.padding(12.dp)) {
                    Text(
                        "Cached online route · ${route.hospital.name} · " +
                            "about ${(route.durationSeconds + 59) / 60} min when fetched · " +
                            "${cachedRouteAgeLabel(cached.fetchedAtEpochMillis, System.currentTimeMillis())}. " +
                            "Readiness and traffic are not current.",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    if (contact?.hotline != null || contact?.smsNumber != null) {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            contact.hotline?.let { number ->
                                TextButton(onClick = { dial(context, number) }) { Text("Call hospital") }
                            }
                            contact.smsNumber?.let { number ->
                                TextButton(
                                    onClick = {
                                        composeHospitalSms(
                                            context,
                                            number,
                                            hospitalReadinessSms(
                                                route.hospital.name,
                                                "general_emergency",
                                                (route.durationSeconds + 59) / 60,
                                            ),
                                        )
                                    },
                                ) { Text("Prepare SMS") }
                            }
                        }
                    }
                }
            }
        }
        Box(Modifier.weight(1f)) { EmergencyScreen(core, settings) }
    }
}

internal fun cachedRouteAgeLabel(fetchedAtMillis: Long, nowMillis: Long): String {
    val elapsedMinutes = ((nowMillis - fetchedAtMillis).coerceAtLeast(0) / 60_000)
    return if (elapsedMinutes < 60) "$elapsedMinutes min old" else "${elapsedMinutes / 60} h old"
}

/**
 * What to say during the first-launch model copy.
 *
 * An estimate is only offered once there is enough of the copy behind it to mean anything.
 * Guessing from the first half second gives a number that then triples, and a countdown that
 * goes up is worse than no countdown: it teaches the reader that the app's own statements
 * about itself cannot be relied on, which is the last thing this app can afford.
 *
 * Rounding is deliberately upward, so the wait ends sooner than promised rather than later.
 * Megabytes are decimal to match the "1.1 GB" the store listing and onboarding already say.
 */
internal fun preparationProgressLabel(
    copiedBytes: Long,
    totalBytes: Long,
    elapsedSeconds: Int,
): String {
    val total = totalBytes.coerceAtLeast(1)
    val copied = copiedBytes.coerceIn(0, total)
    if (copied == 0L) return "Starting…"
    val sizes = "${copied / 1_000_000} MB of ${total / 1_000_000} MB"
    if (elapsedSeconds < MIN_ESTIMATE_SECONDS || copied < MIN_ESTIMATE_BYTES) {
        return "$sizes · estimating"
    }
    val remainingSeconds = (total - copied) * elapsedSeconds / copied
    return when {
        remainingSeconds <= 0 -> "$sizes · finishing"
        remainingSeconds < 60 -> "$sizes · less than a minute left"
        else -> "$sizes · about ${(remainingSeconds + 59) / 60} min left"
    }
}

private const val MIN_ESTIMATE_SECONDS = 3
private const val MIN_ESTIMATE_BYTES = 32_000_000L

private val AppMode.labelRes: Int
    get() =
        when (this) {
            AppMode.OFFLINE -> R.string.mode_offline
            AppMode.ONLINE -> R.string.mode_online
            AppMode.CHAT -> R.string.mode_chat
        }

private val AppMode.shortLabel: String
    get() =
        when (this) {
            AppMode.OFFLINE -> "OFF"
            AppMode.ONLINE -> "ON"
            AppMode.CHAT -> "AI"
        }

private val AppMode.subtitleRes: Int
    get() =
        when (this) {
            AppMode.OFFLINE -> R.string.subtitle_offline
            AppMode.ONLINE -> R.string.subtitle_online
            AppMode.CHAT -> R.string.subtitle_chat
        }

private val AppMode.statusRes: Int
    get() =
        when (this) {
            AppMode.OFFLINE -> R.string.status_offline
            AppMode.ONLINE -> R.string.status_online
            AppMode.CHAT -> R.string.status_chat
        }
