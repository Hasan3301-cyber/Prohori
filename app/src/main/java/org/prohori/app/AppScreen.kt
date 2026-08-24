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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.prohori.core.Prohori

@Composable
fun AppScreen(core: Prohori, settings: Settings) {
    val context = LocalContext.current
    val modelStore = remember { ModelStore(context.applicationContext) }
    var attempt by remember { mutableIntStateOf(0) }
    var modelState by remember {
        mutableStateOf<BundledModelState>(
            if (modelStore.installed()) BundledModelState.Ready else BundledModelState.Preparing,
        )
    }

    LaunchedEffect(attempt) {
        if (modelState == BundledModelState.Ready || modelState == BundledModelState.Skipped) {
            return@LaunchedEffect
        }
        modelState = BundledModelState.Preparing
        val result = withContext(Dispatchers.IO) { runCatching { modelStore.installBundled() } }
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
        BundledModelState.Preparing -> BundledModelPreparation()
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
    Scaffold(
        containerColor = ProhoriCanvas,
        topBar = { ProhoriHeader(mode) },
        bottomBar = {
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
                            label = { Text(choice.label, fontWeight = FontWeight.SemiBold) },
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
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when (mode) {
                AppMode.OFFLINE -> OfflineMode(core, settings, mode)
                AppMode.ONLINE -> OnlineEmergencyScreen(settings)
                AppMode.CHAT -> GeneralChatScreen()
            }
        }
    }
}

@Composable
private fun ProhoriHeader(mode: AppMode) {
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
                Text(mode.subtitle, color = Color.White.copy(alpha = 0.66f), fontSize = 13.sp)
            }
            Surface(
                color = Color.White.copy(alpha = 0.1f),
                shape = RoundedCornerShape(8.dp),
                border = BorderStroke(1.dp, Color.White.copy(alpha = 0.14f)),
            ) {
                Text(
                    mode.status,
                    modifier = Modifier.padding(horizontal = 9.dp, vertical = 6.dp),
                    color = ProhoriWhite,
                    fontSize = 10.sp,
                    fontWeight = FontWeight.Bold,
                    letterSpacing = 0.8.sp,
                )
            }
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
private fun BundledModelPreparation() {
    FirstLaunchFrame(
        title = "Preparing your private AI",
        body =
            "The verified 1.1 GB model is included. Prohori is copying it to private " +
                "storage and checking every byte before first use.",
    ) {
        CircularProgressIndicator(color = ProhoriInk, strokeWidth = 3.dp)
        Spacer(Modifier.height(18.dp))
        Text("KEEP THE APP OPEN", style = MaterialTheme.typography.labelMedium, color = ProhoriMuted)
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
            Column(
                modifier = Modifier.fillMaxSize().padding(horizontal = 28.dp, vertical = 40.dp),
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
    val cached = remember(refreshKey) { OnlineRouteCache(settings).load() }
    Column(Modifier.fillMaxSize()) {
        cached?.routes?.firstOrNull()?.let { route ->
            Surface(
                color = ProhoriGreenSoft,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                shape = RoundedCornerShape(14.dp),
                border = BorderStroke(1.dp, ProhoriGreen.copy(alpha = 0.22f)),
            ) {
                Text(
                    "Cached online route · ${route.hospital.name} · " +
                        "about ${(route.durationSeconds + 59) / 60} min when fetched · " +
                        "${cachedRouteAgeLabel(cached.fetchedAtEpochMillis, System.currentTimeMillis())}. " +
                        "Readiness and traffic are not current.",
                    modifier = Modifier.padding(12.dp),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        Box(Modifier.weight(1f)) { EmergencyScreen(core, settings) }
    }
}

internal fun cachedRouteAgeLabel(fetchedAtMillis: Long, nowMillis: Long): String {
    val elapsedMinutes = ((nowMillis - fetchedAtMillis).coerceAtLeast(0) / 60_000)
    return if (elapsedMinutes < 60) "$elapsedMinutes min old" else "${elapsedMinutes / 60} h old"
}

private val AppMode.label: String
    get() =
        when (this) {
            AppMode.OFFLINE -> "Offline"
            AppMode.ONLINE -> "Online"
            AppMode.CHAT -> "Chat"
        }

private val AppMode.shortLabel: String
    get() =
        when (this) {
            AppMode.OFFLINE -> "OFF"
            AppMode.ONLINE -> "ON"
            AppMode.CHAT -> "AI"
        }

private val AppMode.subtitle: String
    get() =
        when (this) {
            AppMode.OFFLINE -> "Emergency guidance without internet"
            AppMode.ONLINE -> "Live hospital routing and readiness"
            AppMode.CHAT -> "Private conversation on this phone"
        }

private val AppMode.status: String
    get() =
        when (this) {
            AppMode.OFFLINE -> "OFFLINE READY"
            AppMode.ONLINE -> "LIVE MODE"
            AppMode.CHAT -> "ON DEVICE"
        }
