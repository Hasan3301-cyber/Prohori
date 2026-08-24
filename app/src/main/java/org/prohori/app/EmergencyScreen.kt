package org.prohori.app

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import org.prohori.core.CardStep
import org.prohori.core.CityPackInstall
import org.prohori.core.CountryChoice
import org.prohori.core.EmergencyNumbers
import org.prohori.core.FirstAidCard
import org.prohori.core.HospitalConfirmationRequest
import org.prohori.core.HospitalConfirmationResult
import org.prohori.core.HospitalConfirmationStatus
import org.prohori.core.HospitalConfirmationView
import org.prohori.core.HospitalContactChannel
import org.prohori.core.HospitalReply
import org.prohori.core.HospitalReplySource
import org.prohori.core.ModelAssessment
import org.prohori.core.ModelWrittenGuidance
import org.prohori.core.NumberProvenance
import org.prohori.core.OfflineRouteRequest
import org.prohori.core.OfflineRouteResult
import org.prohori.core.Prohori
import org.prohori.core.RecognisedEmergency
import org.prohori.core.SearchResult
import org.prohori.core.StepAction
import org.prohori.core.Triage
import org.prohori.core.Urgency
import org.prohori.core.coreVersion
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * How long the app keeps asking the relay about one awaiting case.
 *
 * Five seconds is fast enough that an operator watching the screen sees the reply land, and
 * slow enough that a five-minute wait is sixty requests rather than three hundred. When the
 * attempts run out the app says it has stopped; it does not expire the request, because
 * `PLAN.md` §7 makes giving up an operator's decision.
 */
private const val HOSPITAL_POLL_INTERVAL_MILLIS = 5_000L
private const val HOSPITAL_POLL_ATTEMPTS = 60

/**
 * How long typing has to stop before the model is woken for an unmatched query.
 *
 * A decode is two to six seconds of CPU in someone's hand, so this is not a debounce for
 * smoothness — it is the difference between one generation and one per keystroke. Six hundred
 * milliseconds is longer than the gap between two letters and shorter than the pause someone
 * makes when they have finished a sentence and are waiting for the screen to answer.
 */
private const val FALLBACK_DEBOUNCE_MILLIS = 600L

/**
 * A model-written answer, and the exact message it was written for.
 *
 * The message travels with the guidance because a decode is slow and typing is not: without
 * it, an answer to "she is very cold" can land on screen under "he is not breathing". The
 * screen renders this only while [forMessage] still matches what is in the field.
 *
 * [note] is why there is nothing to show — a refusal named by Rust, or a native failure. It
 * is never a judgement of our own about the model's text; Kotlin does not read that text.
 */
private data class FallbackShown(
    val forMessage: String,
    val guidance: ModelWrittenGuidance?,
    val note: String?,
)

/**
 * The one screen this build has.
 *
 * # The dialer is the floor, not a feature
 *
 * The call button lives in [Scaffold]'s bottom bar, which means it is on screen before
 * anything has been typed, while the keyboard is open, during every scroll position, and in
 * every state the triage layer can be in — including the state where the app understood
 * nothing. `windowSoftInputMode="adjustResize"` in the manifest is what keeps it above the
 * keyboard instead of behind it.
 *
 * The reasoning: everything else on this screen is guidance, and guidance can be wrong or
 * missing. A phone number is the one thing here that works even when the rest of the app
 * has failed at its job, so it is never more than one tap away and never scrolled past.
 *
 * # There is no view model
 *
 * [Prohori.triage] is a regex sweep over a short string against a table compiled into the
 * library — no I/O, no allocation the caller manages, no failure mode. Calling it from
 * composition on each keystroke is correct here and cheaper than the machinery needed to
 * move it off the main thread. `PLAN.md` §8 budgets 100 ms for a red-flag card; this path
 * is three orders of magnitude inside that. When the model arrives in a later phase it
 * will not be able to make that claim, and it will need somewhere else to run.
 *
 * # No `@Preview`
 *
 * Every composable here takes its content from a loaded [Prohori], which needs
 * `libprohori_ffi.so`. Previews run on the host JVM with no `.so`, so a preview would
 * either crash or have to be fed hand-written fixture text — hand-written medical text
 * living in the UI layer is exactly what the rest of this codebase is built to prevent.
 */
@Composable
// `debounce` is the one preview API in this file. Accepted deliberately: the alternative is
// hand-rolling a timer inside the collector, and a hand-rolled debounce on the path that
// decides whether to wake a model is more likely to be wrong than this annotation is to break.
@OptIn(FlowPreview::class)
fun EmergencyScreen(core: Prohori, settings: Settings) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val modelStore = remember { ModelStore(context.applicationContext) }
    val cityPackStore = remember { CityPackStore(context.applicationContext) }
    val initialCityPack = remember { runCatching { cityPackStore.installActiveOrBundled(core) } }

    // The user's own setting wins over the phone's guess; see `CountryHint`.
    var country by remember { mutableStateOf(settings.country ?: CountryHint.detect(context)) }
    var manualNumber by remember { mutableStateOf(settings.ambulanceOverride) }
    var message by remember { mutableStateOf("") }
    var showSettings by remember { mutableStateOf(false) }
    var dialFailed by remember { mutableStateOf<String?>(null) }
    var modelInstalled by remember { mutableStateOf(modelStore.installed()) }
    var modelBusy by remember { mutableStateOf(false) }
    var modelStatus by remember { mutableStateOf<String?>(null) }
    var modelAssessment by remember { mutableStateOf<ModelAssessment?>(null) }
    var offlineRoute by remember { mutableStateOf<OfflineRouteResult?>(null) }
    var cityPackInstall by remember { mutableStateOf(initialCityPack.getOrNull()) }
    var cityPackError by remember { mutableStateOf(initialCityPack.exceptionOrNull()?.message) }
    var cityPackBusy by remember { mutableStateOf(false) }
    var hospitalConfirmation by remember { mutableStateOf(core.hospitalConfirmation()) }
    var hospitalConfirmationError by remember { mutableStateOf<String?>(null) }
    // Resolved once: which transport, if any, this build can send an online alert with.
    val alertTransport = remember { AlertTransports.resolve() }
    var relayBusy by remember { mutableStateOf(false) }
    var relayNote by remember { mutableStateOf<String?>(null) }
    // The unmatched path. The card is cited and deterministic and costs nothing to hold; the
    // model's own words are held next to the message they were written for.
    val safetyNet = remember { core.safetyNetCard() }
    var fallbackShown by remember { mutableStateOf<FallbackShown?>(null) }
    var fallbackBusy by remember { mutableStateOf(false) }

    val acceptConfirmationResult: (HospitalConfirmationResult) -> Unit = { result ->
        result.confirmation?.let { hospitalConfirmation = it }
        hospitalConfirmationError = result.error
    }

    // Ask the relay about this one case while it is awaiting an answer, and about nothing
    // else. Keyed on the case id so the poll follows the request rather than the screen:
    // Compose cancels it when the case reaches a terminal state or the panel leaves.
    //
    // Polling stops after HOSPITAL_POLL_ATTEMPTS and says so. It does not expire the request
    // — `PLAN.md` §7 leaves that to the operator — but a phone that quietly polls a dead
    // relay for an hour is a phone with no battery left for the call that matters.
    val awaitingOnlineCase =
        hospitalConfirmation
            ?.takeIf {
                it.status == HospitalConfirmationStatus.AWAITING &&
                    it.channel == HospitalContactChannel.ONLINE
            }
            ?.caseId
    LaunchedEffect(awaitingOnlineCase) {
        val caseId = awaitingOnlineCase ?: return@LaunchedEffect
        val transport = alertTransport ?: return@LaunchedEffect
        repeat(HOSPITAL_POLL_ATTEMPTS) {
            delay(HOSPITAL_POLL_INTERVAL_MILLIS)
            val reply = transport.poll(caseId) ?: return@repeat
            acceptConfirmationResult(
                core.ingestOnlineReply(
                    if (reply == RelayReply.YES) HospitalReply.YES else HospitalReply.NO,
                    (System.currentTimeMillis() / 1_000).toULong(),
                    caseId,
                ),
            )
            return@LaunchedEffect
        }
        relayNote =
            "Stopped checking automatically after " +
                "${HOSPITAL_POLL_ATTEMPTS * HOSPITAL_POLL_INTERVAL_MILLIS / 60_000} minutes. " +
                "This hospital is still not confirmed."
    }

    val modelPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri != null) {
                scope.launch {
                    modelBusy = true
                    modelAssessment = null
                    modelStatus = "Checking and copying the GGUF model…"
                    val result =
                        runCatching {
                            withContext(Dispatchers.IO) { modelStore.import(uri) }
                        }
                    result.onSuccess { imported ->
                        modelInstalled = true
                        modelStatus =
                            "Model ready · ${imported.bytes / 1_000_000} MB · " +
                                "SHA-256 ${imported.sha256.take(12)}…"
                    }.onFailure { error ->
                        modelInstalled = modelStore.installed()
                        modelStatus = "Model not installed: ${error.message ?: "unknown error"}"
                    }
                    modelBusy = false
                }
            }
        }

    val cityPackPicker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri != null) {
                scope.launch {
                    cityPackBusy = true
                    offlineRoute = null
                    val result =
                        runCatching {
                            withContext(Dispatchers.IO) { cityPackStore.import(uri, core) }
                        }
                    result.onSuccess { install ->
                        if (install.accepted) {
                            cityPackInstall = install
                            cityPackError = null
                        } else {
                            cityPackError = install.error
                        }
                    }.onFailure { error ->
                        cityPackError = error.message ?: "The selected city pack could not be read"
                    }
                    cityPackBusy = false
                }
            }
        }

    // `remember(key)` is the whole state machine: change the inputs, the core recomputes.
    val numbers: EmergencyNumbers =
        remember(country, manualNumber, cityPackInstall) {
            core.emergencyNumbers(country, manualNumber, null)
        }
    // A blank message returns an empty Triage rather than an error, so this needs no guard.
    val triage: Triage = remember(message) { core.triage(message) }
    // Search is independent of triage. It can suggest a cited reference card, but it can
    // never lower severity or replace the deterministic red-flag card above it.
    val searchResults: List<SearchResult> = remember(message) { core.search(message) }
    // A loose search suggestion is not proof that the corpus covers this situation.
    // Rust requires a complete declared lay phrase before it suppresses the unmatched
    // path; this value also decides which branch is rendered below.
    val fallbackPermitted = remember(message) { core.fallbackPermitted(message) }
    // Empty in a healthy build. A card that failed validation is a card that does not exist
    // on this phone, and that is not something to discover in silence.
    val loadErrors = remember { core.corpusLoadErrors() }

    // When nothing in the corpus covers what was typed, the model writes something itself.
    // No button: someone holding a phone over a casualty is not going to find a second one.
    //
    // Three operators make an automatic decode affordable, and each is load-bearing:
    //
    //  - `debounce` waits for typing to stop, so a half-written word never reaches the model;
    //  - `distinctUntilChanged` over the trimmed text means re-reading what you wrote, or
    //    adding a trailing space, costs nothing;
    //  - `conflate` is the trailing single-flight. A native decode holds `engine_mutex` and
    //    cannot be interrupted, so `collectLatest` would cancel this coroutine and leave the
    //    decode running underneath it — the phone would heat up while the screen showed
    //    nothing. `conflate` keeps only the newest text while a generation is in flight and
    //    runs it once, after the current one returns.
    //
    // `fallbackPermitted` is asked here only to avoid waking the model. Rust asks again after
    // generation, and that second answer is the one that decides — so a red-flag rule that
    // starts matching mid-decode throws the answer away rather than racing it onto the screen.
    //
    // Keyed on `modelInstalled` so importing a model re-offers the text already in the field.
    LaunchedEffect(modelInstalled) {
        if (!modelInstalled) return@LaunchedEffect
        snapshotFlow { message }
            .debounce(FALLBACK_DEBOUNCE_MILLIS)
            .map { it.trim() }
            .distinctUntilChanged()
            .conflate()
            .collect { typed ->
                if (!core.fallbackPermitted(typed)) return@collect
                fallbackBusy = true
                val result =
                    runCatching {
                        withContext(Dispatchers.Default) {
                            OnDeviceEngine.writeFallback(core, modelStore.modelFile, typed)
                        }
                    }
                fallbackShown =
                    result.fold(
                        onSuccess = { run ->
                            FallbackShown(
                                forMessage = typed,
                                guidance = run.assessment.guidance,
                                note = run.assessment.error ?: run.assessment.suppressed,
                            )
                        },
                        onFailure = { error ->
                            FallbackShown(
                                forMessage = typed,
                                guidance = null,
                                note = error.message ?: "the model could not run on this phone",
                            )
                        },
                    )
                fallbackBusy = false
            }
    }

    Scaffold(
        containerColor = ProhoriCanvas,
        bottomBar = {
            DialBar(
                numbers = numbers,
                onDial = { number ->
                    dialFailed = if (dial(context, number)) null else number
                },
                onWrongNumber = { showSettings = true },
            )
        },
    ) { insets ->
        Column(
            modifier =
                Modifier
                    .padding(insets)
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp),
        ) {
            Spacer(Modifier.height(12.dp))

            dialFailed?.let { number ->
                Notice(
                    label = "Dial by hand",
                    body =
                        "This phone did not open its dialer. The number is $number — " +
                            "key it in, or use another phone.",
                    emphasis = true,
                )
            }

            // Absent in a healthy build.
            if (loadErrors.isNotEmpty()) {
                Notice(
                    label = "This build is incomplete",
                    body =
                        "Some guidance did not load and is missing from this app:\n" +
                            loadErrors.joinToString("\n") { "• $it" },
                    emphasis = true,
                )
            }

            Text("OFFLINE TRIAGE", style = MaterialTheme.typography.labelMedium, color = ProhoriRed)
            Spacer(Modifier.height(6.dp))
            Text("Tell us what you see", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(8.dp))
            Text(
                text = "Use plain words. Emergency rules and reviewed guides work with no internet or signal.",
                style = MaterialTheme.typography.bodyMedium,
                color = ProhoriMuted,
            )
            Spacer(Modifier.height(16.dp))

            Surface(
                modifier = Modifier.fillMaxWidth(),
                color = ProhoriWhite,
                shape = RoundedCornerShape(20.dp),
                border = BorderStroke(1.dp, ProhoriBorder),
            ) {
                Column(Modifier.padding(16.dp)) {
                    Text("WHAT IS HAPPENING?", style = MaterialTheme.typography.labelMedium, color = ProhoriMuted)
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = message,
                        onValueChange = { message = it },
                        modifier = Modifier.fillMaxWidth(),
                        textStyle = MaterialTheme.typography.bodyLarge,
                        placeholder = { Text("For example: he is not breathing") },
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Default),
                        minLines = 3,
                        shape = RoundedCornerShape(14.dp),
                    )
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Critical warning signs are checked before local AI is allowed to answer.",
                        style = MaterialTheme.typography.bodySmall,
                        color = ProhoriMuted,
                    )
                }
            }

            Spacer(Modifier.height(16.dp))

            when {
                triage.hits.isNotEmpty() -> Recognised(triage)
                searchResults.isNotEmpty() && !fallbackPermitted ->
                    SearchResults(searchResults, heading = "Matching first-aid guides")
                message.isNotBlank() ->
                    Unmatched(
                        safetyNet = safetyNet,
                        modelInstalled = modelInstalled,
                        // Asked rather than re-derived here. In this branch the rules and the
                        // index have both already come back empty, so the only remaining
                        // reason is the length floor — and a copy of that floor in Kotlin is
                        // a copy that drifts.
                        permitted = fallbackPermitted,
                        busy = fallbackBusy,
                        // The staleness guard: an answer written for older text is not shown.
                        shown = fallbackShown?.takeIf { it.forMessage == message.trim() },
                    )
                else -> Waiting()
            }

            Spacer(Modifier.height(16.dp))
            OnDeviceModelPanel(
                installed = modelInstalled,
                busy = modelBusy,
                status = modelStatus,
                assessment = modelAssessment,
                canAssess = message.isNotBlank(),
                onImport = { modelPicker.launch(arrayOf("application/octet-stream", "*/*")) },
                onAssess = {
                    scope.launch {
                        modelBusy = true
                        modelStatus = "Qwen is extracting bounded triage fields on this phone…"
                        modelAssessment = null
                        val result =
                            runCatching {
                                withContext(Dispatchers.Default) {
                                    OnDeviceEngine.assessWithMetrics(core, modelStore.modelFile, message)
                                }
                            }
                        result.onSuccess { run ->
                            val assessment = run.assessment
                            modelAssessment = assessment
                            modelStatus =
                                if (assessment.accepted) {
                                    "Passed Rust verification · first token ${run.metrics.timeToFirstTokenMillis} ms · " +
                                        "${"%.1f".format(run.metrics.tokensPerSecond)} tokens/s"
                                } else {
                                    "Model output was rejected; deterministic guidance remains active."
                                }
                        }.onFailure { error ->
                            modelStatus = "On-device model unavailable: ${error.message ?: "unknown error"}"
                        }
                        modelBusy = false
                    }
                },
            )

            Spacer(Modifier.height(16.dp))
            OfflineRoutePanel(
                install = cityPackInstall,
                loadError = cityPackError,
                busy = cityPackBusy,
                route = offlineRoute,
                onImport = {
                    cityPackPicker.launch(arrayOf("application/zip", "application/octet-stream", "*/*"))
                },
                onRoute = {
                    offlineRoute =
                        core.offlineRoute(
                            OfflineRouteRequest(
                                latitude = 24.3630,
                                longitude = 88.6280,
                                specialty =
                                    modelAssessment?.specialty?.name?.lowercase()
                                        ?: "general_emergency",
                                nowEpochSeconds = (System.currentTimeMillis() / 1_000).toULong(),
                                vehicleWidthMillimetres = 2_400u,
                                vehicleHeightMillimetres = 3_000u,
                                permitFloodedOriginZone = false,
                            ),
                        )
                },
            )

            offlineRoute?.takeIf { it.accepted }?.let { route ->
                Spacer(Modifier.height(16.dp))
                HospitalConfirmationPanel(
                    route = route,
                    confirmation = hospitalConfirmation,
                    error = hospitalConfirmationError,
                    transportLabel = alertTransport?.label,
                    transportUnavailable =
                        if (alertTransport == null) AlertTransports.unavailableReason() else null,
                    relayBusy = relayBusy,
                    relayNote = relayNote,
                    onStart = { channel ->
                        val hospitalId = route.hospitalId
                        val etaSeconds = route.estimatedSeconds
                        if (hospitalId == null || etaSeconds == null) {
                            hospitalConfirmationError = "The verified route has no hospital or ETA."
                        } else {
                            val etaMinutes = ((etaSeconds + 59uL) / 60uL).coerceAtLeast(1uL)
                            val result =
                                core.startHospitalConfirmation(
                                    HospitalConfirmationRequest(
                                        hospitalId = hospitalId,
                                        specialty =
                                            modelAssessment?.specialty?.name?.lowercase()
                                                ?: "general_emergency",
                                        etaMinutes = etaMinutes.coerceAtMost(UInt.MAX_VALUE.toULong()).toUInt(),
                                        channel = channel,
                                        createdAtEpochMillis = System.currentTimeMillis().toULong(),
                                    ),
                                )
                            acceptConfirmationResult(result)
                        }
                    },
                    onSendOnline = { active ->
                        val body = active.onlineBody
                        val transport = alertTransport
                        if (body == null || transport == null) {
                            hospitalConfirmationError =
                                "This build cannot send an online alert. Use SMS or the hotline."
                        } else {
                            relayBusy = true
                            relayNote = null
                            hospitalConfirmationError = null
                            scope.launch {
                                val outcome =
                                    transport.send(
                                        HospitalAlert(
                                            caseId = active.caseId,
                                            hospitalId = active.hospitalId,
                                            telegramChatId = active.destination,
                                            specialty = active.specialty,
                                            etaMinutes = active.etaMinutes,
                                            body = body,
                                        ),
                                    )
                                relayBusy = false
                                when (outcome) {
                                    // The app sent this itself, so there is no "I sent it"
                                    // attestation to ask for — unlike the SMS path, where the
                                    // app genuinely cannot see whether Send was tapped.
                                    is SendOutcome.Sent ->
                                        acceptConfirmationResult(
                                            core.markHospitalContacted(
                                                (System.currentTimeMillis() / 1_000).toULong(),
                                            ),
                                        )
                                    is SendOutcome.Refused -> {
                                        hospitalConfirmationError =
                                            "Nothing was sent: ${outcome.reason}"
                                    }
                                }
                            }
                        }
                    },
                    onOpenSms = { destination, body ->
                        hospitalConfirmationError =
                            if (composeHospitalSms(context, destination, body)) {
                                null
                            } else {
                                "No SMS app opened. Copy the message or use the voice channel."
                            }
                    },
                    onOpenVoice = { destination ->
                        hospitalConfirmationError =
                            if (dial(context, destination)) null else "No dialer opened. Dial $destination by hand."
                    },
                    onCopy = { label, text ->
                        hospitalConfirmationError =
                            if (copyText(context, label, text)) null else "Could not copy the script."
                    },
                    onContacted = {
                        acceptConfirmationResult(
                            core.markHospitalContacted(
                                (System.currentTimeMillis() / 1_000).toULong(),
                            ),
                        )
                    },
                    onReply = { reply ->
                        acceptConfirmationResult(
                            core.recordHospitalReply(
                                reply,
                                (System.currentTimeMillis() / 1_000).toULong(),
                                "device operator",
                            ),
                        )
                    },
                    onExpire = {
                        acceptConfirmationResult(
                            core.expireHospitalConfirmation(
                                (System.currentTimeMillis() / 1_000).toULong(),
                            ),
                        )
                    },
                )
            }

            if (triage.hits.isNotEmpty()) {
                val primaryId = triage.card?.protocolId
                val otherResults = searchResults.filter { it.card.protocolId != primaryId }
                if (otherResults.isNotEmpty()) {
                    Spacer(Modifier.height(16.dp))
                    SearchResults(otherResults, heading = "Other matching guides")
                }
            }

            Spacer(Modifier.height(24.dp))
            HorizontalDivider()
            AllCards(core)
            HorizontalDivider()
            About(numbers)
            Spacer(Modifier.height(24.dp))
        }
    }

    if (showSettings) {
        SettingsDialog(
            core = core,
            current = numbers,
            initialOverride = manualNumber.orEmpty(),
            onDismiss = { showSettings = false },
            onApply = { chosenCountry, typedOverride ->
                settings.country = chosenCountry
                settings.ambulanceOverride = typedOverride
                country = chosenCountry
                manualNumber = typedOverride?.takeIf { it.isNotBlank() }
                showSettings = false
            },
        )
    }
}

/** P3 demo: every route claim is visibly tied to pack freshness and field-check state. */
@Composable
private fun OfflineRoutePanel(
    install: CityPackInstall?,
    loadError: String?,
    busy: Boolean,
    route: OfflineRouteResult?,
    onImport: () -> Unit,
    onRoute: () -> Unit,
) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
    ) {
        Column(Modifier.padding(14.dp)) {
            Text("Signed offline route demo", style = MaterialTheme.typography.titleMedium)
            Text(
                "RUET gate → Rajshahi Medical College Hospital. No internet or map server is used.",
                style = MaterialTheme.typography.bodyMedium,
            )
            Spacer(Modifier.height(8.dp))
            loadError?.let {
                Notice("Pack update refused", it, emphasis = true)
                Spacer(Modifier.height(6.dp))
            }
            when {
                install == null || !install.accepted ->
                    Notice(
                        "City pack refused",
                        install?.error ?: "The bundled pack did not load.",
                        emphasis = true,
                    )
                else -> {
                    if (!install.fieldChecked) {
                        Notice(
                            "DEMO — not field checked",
                            "The signature and file hashes pass, but this topology must not be used for real navigation.",
                            emphasis = true,
                        )
                    }
                    Spacer(Modifier.height(6.dp))
                    Button(onClick = onRoute, enabled = !busy) { Text("Calculate offline demo route") }
                    TextButton(onClick = onImport, enabled = !busy) {
                        Text(if (busy) "Checking pack…" else "Import signed pack update")
                    }
                }
            }
            route?.let { result ->
                Spacer(Modifier.height(8.dp))
                if (!result.accepted) {
                    Notice(
                        "Route refused safely",
                        result.error ?: "No route has fresh, known conditions.",
                        emphasis = true,
                    )
                } else {
                    Text(
                        "${result.hospitalName} · about ${(result.estimatedSeconds ?: 0uL) / 60uL} min",
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Text(
                        "Road snapshot age: ${result.conditionAgeSeconds ?: 0uL}s · " +
                            "facility data age: ${result.facilityAgeSeconds ?: 0uL}s",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Text(
                        "Conditions: ${result.conditionSources.joinToString()} · edges ${result.edgeIds.joinToString()}",
                        style = MaterialTheme.typography.bodySmall,
                    )
                    result.attribution?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
}

/** P4: a contact attempt is not readiness; only a recorded explicit YES is readiness. */
@Composable
private fun HospitalConfirmationPanel(
    route: OfflineRouteResult,
    confirmation: HospitalConfirmationView?,
    error: String?,
    /** Which transport would send, named so a drill is never mistaken for live dispatch. */
    transportLabel: String?,
    /** Why the online channel is unavailable, when it is. Null when a transport exists. */
    transportUnavailable: String?,
    relayBusy: Boolean,
    relayNote: String?,
    onStart: (HospitalContactChannel) -> Unit,
    onSendOnline: (HospitalConfirmationView) -> Unit,
    onOpenSms: (String, String) -> Unit,
    onOpenVoice: (String) -> Unit,
    onCopy: (String, String) -> Unit,
    onContacted: () -> Unit,
    onReply: (HospitalReply) -> Unit,
    onExpire: () -> Unit,
) {
    val isCurrentDestination =
        confirmation != null &&
            confirmation.packId == route.packId &&
            confirmation.hospitalId == route.hospitalId
    val terminal =
        confirmation?.status in
            setOf(
                HospitalConfirmationStatus.CONFIRMED,
                HospitalConfirmationStatus.DECLINED,
                HospitalConfirmationStatus.EXPIRED,
            )

    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
    ) {
        Column(Modifier.padding(14.dp)) {
            Text("Confirm hospital availability", style = MaterialTheme.typography.titleMedium)
            Text(
                "Opening Messages or the dialer does not confirm a bed. Record only what a hospital operator explicitly says.",
                style = MaterialTheme.typography.bodyMedium,
            )
            error?.let {
                Spacer(Modifier.height(8.dp))
                Notice("Contact action not completed", it, emphasis = true)
            }

            if (confirmation == null || terminal) {
                Spacer(Modifier.height(8.dp))
                if (route.hospitalTelegram != null && transportUnavailable == null) {
                    Button(onClick = { onStart(HospitalContactChannel.ONLINE) }) {
                        Text("Prepare Telegram alert")
                    }
                    Spacer(Modifier.height(4.dp))
                }
                route.hospitalSms?.let {
                    Button(onClick = { onStart(HospitalContactChannel.SMS_INTENT) }) {
                        Text("Prepare registered SMS")
                    }
                    Spacer(Modifier.height(4.dp))
                }
                if (route.hospitalHotline != null) {
                    OutlinedButton(onClick = { onStart(HospitalContactChannel.VOICE) }) {
                        Text("Prepare voice call")
                    }
                }
                if (route.hospitalSms == null) {
                    Text(
                        "This signed pack has no registered SMS endpoint; voice remains available.",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                // Two different absences, and conflating them would send someone looking for
                // the wrong fix: the pack may bind no chat, or this build may have no relay.
                if (route.hospitalTelegram == null) {
                    Text(
                        "This signed pack has no registered Telegram chat for this hospital.",
                        style = MaterialTheme.typography.bodySmall,
                    )
                } else if (transportUnavailable != null) {
                    Text(transportUnavailable, style = MaterialTheme.typography.bodySmall)
                }
            }

            confirmation?.let { active ->
                Spacer(Modifier.height(8.dp))
                if (!isCurrentDestination) {
                    Notice(
                        "Different active request",
                        "Finish this ${active.hospitalName} request before starting another destination.",
                        emphasis = true,
                    )
                }
                Text(
                    "${active.hospitalName} · case ${active.caseId}",
                    style = MaterialTheme.typography.titleSmall,
                )
                when (active.status) {
                    HospitalConfirmationStatus.DRAFT -> {
                        active.onlineBody?.let { body ->
                            Text("Will be sent to ${active.destination}:")
                            Text(body, style = MaterialTheme.typography.bodyMedium)
                            Text(
                                "Only the case, the specialty, and the ETA leave this device. " +
                                    "No symptoms, no location, no name.",
                                style = MaterialTheme.typography.bodySmall,
                            )
                            transportLabel?.let {
                                Text("Sending via $it", style = MaterialTheme.typography.bodySmall)
                            }
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Button(
                                    onClick = { onSendOnline(active) },
                                    enabled = !relayBusy,
                                ) {
                                    Text(if (relayBusy) "Sending…" else "Send now")
                                }
                                TextButton(onClick = { onCopy("Hospital alert", body) }) {
                                    Text("Copy")
                                }
                            }
                        }
                        active.smsBody?.let { body ->
                            Text(body, style = MaterialTheme.typography.bodyMedium)
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Button(onClick = { onOpenSms(active.destination, body) }) {
                                    Text("Open SMS app")
                                }
                                TextButton(onClick = { onCopy("Hospital SMS", body) }) {
                                    Text("Copy")
                                }
                            }
                            Text(
                                "The app cannot see whether you tapped Send. Return only after sending it.",
                                style = MaterialTheme.typography.bodySmall,
                            )
                            OutlinedButton(onClick = onContacted) { Text("I sent this message") }
                        }
                        active.voiceScript?.let { script ->
                            Text("Say exactly:", fontWeight = FontWeight.Bold)
                            Text(script, style = MaterialTheme.typography.bodyMedium)
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Button(onClick = { onOpenVoice(active.destination) }) {
                                    Text("Open dialer")
                                }
                                TextButton(onClick = { onCopy("Hospital voice script", script) }) {
                                    Text("Copy script")
                                }
                            }
                            OutlinedButton(onClick = onContacted) {
                                Text("I asked this question")
                            }
                        }
                    }
                    HospitalConfirmationStatus.AWAITING -> {
                        val online = active.channel == HospitalContactChannel.ONLINE
                        Notice(
                            "NOT CONFIRMED — waiting for an answer",
                            if (online) {
                                "The alert was delivered. Checking for the hospital's reply. " +
                                    "Delivery is not an answer, and silence is not YES."
                            } else {
                                "Look at the SMS reply or listen to the hospital operator. Silence is not YES."
                            },
                            emphasis = true,
                        )
                        relayNote?.let {
                            Spacer(Modifier.height(6.dp))
                            Notice("Automatic checking stopped", it, emphasis = true)
                        }
                        if (online) {
                            Text(
                                "If the operator reads the reply in Telegram before the app does, " +
                                    "record it here.",
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                        Spacer(Modifier.height(6.dp))
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(onClick = { onReply(HospitalReply.YES) }) {
                                Text("Hospital said YES")
                            }
                            OutlinedButton(onClick = { onReply(HospitalReply.NO) }) {
                                Text("Hospital said NO")
                            }
                        }
                        TextButton(onClick = onExpire) { Text("No answer — stop waiting") }
                    }
                    HospitalConfirmationStatus.CONFIRMED ->
                        Notice(
                            if (isCurrentDestination) {
                                "Hospital explicitly said YES"
                            } else {
                                "Previous request: ${active.hospitalName} said YES"
                            },
                            if (!isCurrentDestination) {
                                "This answer does not confirm the currently displayed destination."
                            } else {
                                // Never "recorded by the relay" phrased as a person. A machine
                                // matched a message to this case id; a person heard a voice.
                                // Those are different kinds of evidence and the screen says which.
                                when (active.replySource) {
                                    HospitalReplySource.ONLINE_RELAY ->
                                        "A Telegram reply matched to case ${active.caseId} said YES. " +
                                            "No person on this device heard it. Proceed using the " +
                                            "verified route and keep monitoring the patient."
                                    else ->
                                        "Recorded by ${active.recordedBy ?: "device operator"}. " +
                                            "Proceed using the verified route and keep monitoring the patient."
                                }
                            },
                            emphasis = true,
                        )
                    HospitalConfirmationStatus.DECLINED ->
                        Notice(
                            "Hospital said NO",
                            if (active.replySource == HospitalReplySource.ONLINE_RELAY) {
                                "A Telegram reply matched to case ${active.caseId} said NO. " +
                                    "Do not describe this destination as ready. Contact another verified facility."
                            } else {
                                "Do not describe this destination as ready. Contact another verified facility."
                            },
                            emphasis = true,
                        )
                    HospitalConfirmationStatus.EXPIRED ->
                        Notice(
                            "No explicit answer",
                            "This request expired and the hospital is not confirmed ready.",
                            emphasis = true,
                        )
                }
            }
        }
    }
}

/**
 * P2 model controls. The output is deliberately secondary to deterministic triage above.
 * A rejected or unavailable model never removes a card or changes the dial path.
 */
@Composable
private fun OnDeviceModelPanel(
    installed: Boolean,
    busy: Boolean,
    status: String?,
    assessment: ModelAssessment?,
    canAssess: Boolean,
    onImport: () -> Unit,
    onAssess: () -> Unit,
) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
    ) {
        Column(Modifier.padding(14.dp)) {
            Text("Optional on-device Qwen", style = MaterialTheme.typography.titleMedium)
            Text(
                "The model only extracts fields. Rust rules decide urgency and which cited card is shown.",
                style = MaterialTheme.typography.bodyMedium,
            )
            Spacer(Modifier.height(8.dp))
            if (installed) {
                Button(onClick = onAssess, enabled = canAssess && !busy) {
                    Text(if (busy) "Working…" else "Analyze on this phone")
                }
                Spacer(Modifier.height(4.dp))
                TextButton(onClick = onImport, enabled = !busy) { Text("Replace GGUF") }
            } else {
                OutlinedButton(onClick = onImport, enabled = !busy) {
                    Text(if (busy) "Importing…" else "Import Qwen3 1.7B GGUF")
                }
                Text(
                    "Weights stay in private app storage and are never uploaded.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            status?.let {
                Spacer(Modifier.height(6.dp))
                Text(it, style = MaterialTheme.typography.bodyMedium)
            }
            assessment?.let { result ->
                Spacer(Modifier.height(8.dp))
                if (!result.accepted) {
                    Notice(
                        label = "Model answer refused",
                        body = result.error ?: "It did not match the required structure.",
                        emphasis = true,
                    )
                } else {
                    Text(
                        "Recognised symptoms: " +
                            result.symptoms.ifEmpty { listOf("none stated") }.joinToString(", "),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    result.card?.let {
                        Spacer(Modifier.height(8.dp))
                        ProtocolCard(it)
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The dial bar
// ---------------------------------------------------------------------------

/**
 * The bottom bar. Always present, in every state.
 *
 * Three things, in descending order of how often they are the right answer:
 *
 * 1. The ambulance number, as a target big enough to hit without looking.
 * 2. `112`, when [EmergencyNumbers.gsm112AlsoWorks] and it is not already the number
 *    above. It reaches an operator on any GSM network, even with no SIM and no credit, and
 *    it is the answer when the number above turns out to be wrong for where the user is.
 * 3. The caveat, when [EmergencyNumbers.confirmedLocal] is false. Saying "we think this is
 *    the number" costs a line of text; presenting a guess as a fact costs whatever the
 *    wrong number costs.
 */
@Composable
private fun DialBar(
    numbers: EmergencyNumbers,
    onDial: (String) -> Unit,
    onWrongNumber: () -> Unit,
) {
    Surface(
        color = ProhoriPaper,
        tonalElevation = 0.dp,
        shadowElevation = 14.dp,
        border = BorderStroke(1.dp, ProhoriBorder),
    ) {
        Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
            Button(
                onClick = { onDial(numbers.ambulanceDial) },
                modifier = Modifier.fillMaxWidth().heightIn(min = 72.dp),
                shape = RoundedCornerShape(16.dp),
                colors =
                    ButtonDefaults.buttonColors(
                        containerColor = ProhoriRed,
                        contentColor = ProhoriWhite,
                    ),
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(
                        text = "Call an ambulance",
                        style = MaterialTheme.typography.labelLarge,
                    )
                    Text(
                        text = numbers.ambulance,
                        style = MaterialTheme.typography.titleLarge,
                    )
                }
            }

            if (numbers.gsm112AlsoWorks && numbers.ambulanceDial != "112") {
                Spacer(Modifier.height(8.dp))
                OutlinedButton(
                    onClick = { onDial("112") },
                    modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp),
                    shape = RoundedCornerShape(14.dp),
                ) {
                    Text("Or call 112 — works on any network")
                }
            }

            Spacer(Modifier.height(6.dp))
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    text = numbers.provenanceLine(),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = onWrongNumber) { Text("Change") }
            }
        }
    }
}

/**
 * Where this number came from, in words.
 *
 * The provenance enum exists so this line can be honest. `BUILT_IN` means "the national
 * number for the country we think you are in", which is a different claim from "your local
 * ambulance service", and a user who knows the difference can correct it.
 */
private fun EmergencyNumbers.provenanceLine(): String {
    val place = countryName ?: country
    return when (provenance) {
        NumberProvenance.USER_OVERRIDE -> "You set this number."
        NumberProvenance.CITY_PACK -> "Local service" + (place?.let { " · $it" } ?: "") + "."
        NumberProvenance.BUILT_IN ->
            "National number for ${place ?: "this country"}. Not checked for your city."
        NumberProvenance.GSM_FALLBACK ->
            "We could not tell which country this phone is in. 112 reaches an operator."
    }
}

// ---------------------------------------------------------------------------
// Triage results
// ---------------------------------------------------------------------------

@Composable
private fun Recognised(triage: Triage) {
    triage.severity?.let { severity ->
        SeverityBanner(severity)
        Spacer(Modifier.height(12.dp))
    }

    // A rule fired and no protocol is written for it yet. Saying nothing here would read as
    // "nothing found", which is the opposite of what happened.
    if (triage.recognisedWithoutGuidance.isNotEmpty()) {
        Notice(
            label = "Serious, and we have no steps for it",
            body =
                "This looks like an emergency, but this app has no instructions for it " +
                    "yet. Call now and describe what you see:\n" +
                    triage.recognisedWithoutGuidance.joinToString("\n") { "• ${it.matched}" },
            emphasis = true,
        )
    }

    triage.card?.let { card ->
        ProtocolCard(card)
        Spacer(Modifier.height(12.dp))
    }

    Trace(triage.hits)
}

@Composable
private fun SeverityBanner(severity: Urgency) {
    val critical = severity == Urgency.CRITICAL || severity == Urgency.URGENT
    Surface(
        color =
            if (critical) {
                ProhoriRed
            } else {
                MaterialTheme.colorScheme.surfaceVariant
            },
        contentColor =
            if (critical) {
                ProhoriWhite
            } else {
                MaterialTheme.colorScheme.onSurfaceVariant
            },
        shape = RoundedCornerShape(12.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(14.dp)) {
            Text(text = severity.headline(), style = MaterialTheme.typography.titleLarge)
            Text(text = severity.detail(), style = MaterialTheme.typography.bodyMedium)
        }
    }
}

// The words, not the colour, carry this. A colour-blind user in daylight sees the same
// message as everyone else.
private fun Urgency.headline(): String =
    when (this) {
        Urgency.SELF_CARE -> "You can manage this at home"
        Urgency.STANDARD -> "See a doctor about this"
        Urgency.URGENT -> "Get to a hospital now"
        Urgency.CRITICAL -> "Call for help now"
    }

private fun Urgency.detail(): String =
    when (this) {
        Urgency.SELF_CARE -> "Watch for it getting worse."
        Urgency.STANDARD -> "Not tonight, but do not leave it."
        Urgency.URGENT -> "Hours matter. Start moving."
        Urgency.CRITICAL -> "Call, then start the steps below."
    }

/**
 * Nothing in this app covers what was typed.
 *
 * The order here is the safety argument, not a layout preference. The cited card comes first
 * and arrives with the keystroke: it is what this app has to say about a casualty it cannot
 * name, it is the same card with or without a model on the phone, and it means the screen is
 * never blank while something slow happens. The model's own words come second, in a block
 * that says whose words they are. The browse list stays last.
 *
 * What this must never do is read as reassurance. Nothing matching means the rule table has
 * no phrase for what was typed. It says nothing at all about the patient.
 */
@Composable
private fun Unmatched(
    safetyNet: FirstAidCard?,
    modelInstalled: Boolean,
    permitted: Boolean,
    busy: Boolean,
    shown: FallbackShown?,
) {
    Text(
        text = "We do not have a guide for this",
        style = MaterialTheme.typography.titleMedium,
        fontWeight = FontWeight.Bold,
        color = MaterialTheme.colorScheme.onSurface,
    )
    Text(
        text =
            "That does not mean it is not serious — this app only knows a small number of " +
                "emergencies. If you are worried, call. Until help arrives, this is what " +
                "holds for almost anyone who is hurt.",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Spacer(Modifier.height(8.dp))

    if (safetyNet == null) {
        // Only reachable in a build whose safety-net card failed validation, the same way
        // `loadErrors` is only non-empty in a broken build. Saying so beats a blank space.
        Notice(
            label = "This build is incomplete",
            body =
                "The general guidance for an emergency this app cannot name did not load. " +
                    "Call for help and do not wait for this screen.",
            emphasis = true,
        )
    } else {
        Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            shape = RoundedCornerShape(12.dp),
            modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp),
        ) {
            Column(Modifier.padding(14.dp)) { ProtocolCard(safetyNet) }
        }
    }

    when {
        shown?.guidance != null -> ModelWrittenBlock(shown.guidance)
        busy ->
            Text(
                text =
                    "The model on this phone is writing something for this. Nothing above " +
                        "is waiting on it.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        // Named, not hidden. A refusal is the design working, and a trace that says which
        // check refused is what makes it possible to fix the prompt rather than guess at it.
        shown?.note != null ->
            Notice(
                label = "Nothing more from the model",
                body =
                    "The model wrote an answer and it was not used: ${shown.note}. " +
                        "The guidance above does not depend on it.",
                emphasis = false,
            )
        permitted && !modelInstalled ->
            Text(
                text =
                    "The model on this phone could write more for a case like this. None is " +
                        "installed yet — there is a button for that below.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        else -> Unit
    }

    Spacer(Modifier.height(4.dp))
    Text(
        text = "You can also look through everything this app knows, below.",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

/**
 * The model's own words, in a block that says so.
 *
 * Deliberately not [ProtocolCard] and deliberately not styled like one. A [ProtocolCard]
 * carries numbered steps, named sources and a clinician-review line, and every one of those
 * would be a lie about this text. The label sits inside the block, above the words, because
 * a footnote is the first thing that leaves a small screen.
 *
 * There is no share button either. Cited guidance can be forwarded to someone who is not
 * looking at this screen; unreviewed text written by a model on one phone cannot, because
 * the only thing marking it as such is the screen it is sitting on.
 *
 * Nothing here is checked in Kotlin. `data/grammar/fallback.gbnf` makes a digit
 * unrepresentable while the tokens are being sampled, and `core/src/fallback.rs` refuses a
 * drug, a spelled-out number, an invasive instruction, or a reading grade above six. This
 * composable's whole job is to not pretend the result is something better than it is.
 */
@Composable
private fun ModelWrittenBlock(guidance: ModelWrittenGuidance) {
    Surface(
        color = MaterialTheme.colorScheme.surface,
        contentColor = MaterialTheme.colorScheme.onSurface,
        shape = RoundedCornerShape(12.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
        modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp),
    ) {
        Column(Modifier.padding(14.dp)) {
            Text(
                text = "Written by the model on this phone",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                // Authored in Rust so this sentence has one home; see `fallback::DISCLAIMER`.
                text = guidance.disclaimer,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(10.dp))
            Text(text = guidance.reassurance, style = MaterialTheme.typography.bodyLarge)
            guidance.steps.forEach { step ->
                Row(modifier = Modifier.fillMaxWidth().padding(top = 7.dp)) {
                    // A bullet, not a number: numbered steps are what a cited card has.
                    Text(
                        text = "•",
                        style = MaterialTheme.typography.bodyLarge,
                        modifier = Modifier.width(20.dp),
                    )
                    Text(
                        text = step,
                        style = MaterialTheme.typography.bodyLarge,
                        modifier = Modifier.weight(1f),
                    )
                }
            }
            if (guidance.doNot.isNotEmpty()) {
                Spacer(Modifier.height(10.dp))
                Text(
                    text = "Do not",
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.Bold,
                )
                guidance.doNot.forEach { warning ->
                    Text(text = "• $warning", style = MaterialTheme.typography.bodyLarge)
                }
            }
            if (guidance.callNow) {
                Spacer(Modifier.height(10.dp))
                Text(
                    text = "Keep trying to reach help. The call button below always works.",
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.Bold,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
        }
    }
}

@Composable
private fun Waiting() {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = ProhoriWhite,
        shape = RoundedCornerShape(16.dp),
        border = BorderStroke(1.dp, ProhoriBorder),
    ) {
        Column(Modifier.padding(16.dp)) {
            Text("Ready when you are", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(4.dp))
            Text(
                text =
                    "Describe the symptom above, or call immediately with the fixed button below. " +
                        "You never need to type before calling.",
                style = MaterialTheme.typography.bodyMedium,
                color = ProhoriMuted,
            )
        }
    }
}

/**
 * Why a card appeared.
 *
 * A card that arrives unexplained is a card a frightened person cannot sanity-check. The
 * matched phrase is shown so someone who typed "my chest is not hurting" can see the app
 * matched on "chest" and stop trusting the card in front of them.
 */
@Composable
private fun Trace(hits: List<RecognisedEmergency>) {
    if (hits.isEmpty()) return
    Text(
        text = "Shown because you wrote: " + hits.joinToString(", ") { "“${it.matched}”" },
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurface,
    )
}

/**
 * Deterministic BM25 results. These are reference matches, never a diagnosis or triage.
 * Every expanded result is the full Rust-rendered card, including sources and the
 * clinician-review disclosure.
 */
@Composable
private fun SearchResults(results: List<SearchResult>, heading: String) {
    var openId by remember(results) {
        mutableStateOf<String?>(results.firstOrNull()?.card?.protocolId)
    }

    Text(
        text = heading,
        style = MaterialTheme.typography.titleMedium,
        fontWeight = FontWeight.Bold,
        color = MaterialTheme.colorScheme.onSurface,
    )
    Text(
        text = "Reference matches only — they do not decide how urgent this is.",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Spacer(Modifier.height(8.dp))

    results.forEach { result ->
        val card = result.card
        OutlinedButton(
            onClick = { openId = if (openId == card.protocolId) null else card.protocolId },
            modifier = Modifier.fillMaxWidth().padding(vertical = 3.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text(
                    text = card.title,
                    style = MaterialTheme.typography.bodyLarge,
                    fontWeight = FontWeight.SemiBold,
                )
                if (result.matched.isNotEmpty()) {
                    Text(
                        text = "Matched: ${result.matched.joinToString(", ")}",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        }
        if (openId == card.protocolId) {
            Surface(
                color = MaterialTheme.colorScheme.surfaceVariant,
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier.fillMaxWidth().padding(bottom = 10.dp),
            ) {
                Column(Modifier.padding(14.dp)) { ProtocolCard(card) }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A card
// ---------------------------------------------------------------------------

@Composable
private fun ProtocolCard(card: FirstAidCard) {
    val context = LocalContext.current
    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = ProhoriWhite,
        shape = RoundedCornerShape(20.dp),
        border = BorderStroke(1.dp, ProhoriBorder),
    ) {
      Column(Modifier.fillMaxWidth().padding(18.dp)) {
        Text(
            text = card.title,
            style = MaterialTheme.typography.titleLarge,
            color = MaterialTheme.colorScheme.onSurface,
        )
        if (card.appliesTo.isNotBlank()) {
            Text(
                // Above the steps on purpose: someone who arrived here by mistake should
                // find that out before doing something to a patient.
                text = "For: ${card.appliesTo}",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }

        Spacer(Modifier.height(12.dp))
        card.steps.forEach { step -> StepRow(step) }

        if (card.doNot.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            Notice(
                label = "Do not",
                // Verbatim from the corpus. The verifier cannot catch a flipped negation,
                // so nothing rewrites these lines — not the model, and not this file.
                body = card.doNot.joinToString("\n") { "• $it" },
                emphasis = true,
            )
        }

        if (card.escalateIf.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            Notice(
                label = "Call again if",
                body = card.escalateIf.joinToString("\n") { "• $it" },
                emphasis = false,
            )
        }

        Spacer(Modifier.height(12.dp))
        // `docs/CONVENTIONS.md` §9. The sentence is not written here — it comes from
        // `prohori_core::render::provenance`, which is also what travels inside
        // `card.plainText` when someone shares this card out of the app. Two versions of a
        // statement about who has and has not checked medical instructions is one too many,
        // and the one that would drift is the one in the UI.
        Text(
            text = card.provenance,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = if (card.clinicallyReviewed) FontWeight.Normal else FontWeight.SemiBold,
            color =
                if (card.clinicallyReviewed) {
                    MaterialTheme.colorScheme.onSurface
                } else {
                    MaterialTheme.colorScheme.error
                },
        )

        if (card.sources.isNotEmpty()) {
            // One line per source rather than a joined sentence. A citation exists to be
            // checked, and a reader who cannot pick one out of the list cannot check it.
            Spacer(Modifier.height(8.dp))
            Text(
                text = if (card.sources.size == 1) "Source" else "Sources",
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            card.sources.forEach { source ->
                Text(
                    text = "• $source",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 2.dp),
                )
            }
        }

        // Sends `card.plainText`, which carries the sources and the review status with it.
        // Someone reading these steps in a text message cannot see the screen they were
        // shared from, so the text has to say for itself where it came from — that is the
        // whole design of `prohori_core::render`.
        //
        // Below the sources rather than beside the title: nothing on this card should
        // compete for attention with the steps, and sharing is what you do after the
        // emergency, or for someone who is somewhere else.
        Spacer(Modifier.height(12.dp))
        OutlinedButton(
            onClick = { shareCardText(context, card.title, card.plainText) },
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(text = "Send these steps to someone")
        }
      }
    }
}

/**
 * One step.
 *
 * The kind is shown as a word — "Check", "Do", "Get help" — because the distinction is
 * load-bearing. `data/firstaid/SCHEMA.md` forbids a protocol opening with an action, so
 * the first line a user reads is always something to observe, never something to do to a
 * patient who has not been looked at yet. Rendering that distinction as a colour alone
 * would throw away the invariant the corpus tests exist to protect.
 */
@Composable
private fun StepRow(step: CardStep) {
    Row(modifier = Modifier.fillMaxWidth().padding(vertical = 7.dp)) {
        Text(
            text = "${step.number}",
            style = MaterialTheme.typography.titleLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.width(34.dp),
        )
        Column(Modifier.weight(1f)) {
            Text(
                text = step.kind.label(),
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Bold,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(
                text = step.text,
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }
    }
}

private fun StepAction.label(): String =
    when (this) {
        StepAction.ASSESSMENT -> "CHECK"
        StepAction.ACTION -> "DO"
        StepAction.ESCALATION -> "GET HELP"
    }

// ---------------------------------------------------------------------------
// Browse, About, Settings
// ---------------------------------------------------------------------------

/**
 * Everything in this build, listed.
 *
 * Two reasons this is not just a search box. Someone who cannot spell what is happening
 * can still recognise it in a list; and a user is entitled to know the app's whole scope
 * before an emergency, rather than discovering its limits during one.
 */
@Composable
private fun AllCards(core: Prohori) {
    var expanded by remember { mutableStateOf(false) }
    val cards = remember { core.cards() }
    var openCard by remember { mutableStateOf<FirstAidCard?>(null) }

    TextButton(onClick = { expanded = !expanded }) {
        Text(
            if (expanded) {
                "Hide what this app knows"
            } else {
                "What this app knows (${cards.size})"
            },
        )
    }
    if (expanded) {
        cards.forEach { card ->
            TextButton(
                onClick = {
                    openCard = if (openCard?.protocolId == card.protocolId) null else card
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(
                    text = card.title,
                    modifier = Modifier.weight(1f),
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
            if (openCard?.protocolId == card.protocolId) {
                Column(Modifier.padding(bottom = 16.dp)) { ProtocolCard(card) }
            }
        }
    }
}

@Composable
private fun About(numbers: EmergencyNumbers) {
    Column(Modifier.fillMaxWidth().padding(vertical = 12.dp)) {
        Text(
            text =
                "This is first aid, not a diagnosis. It cannot examine anyone. When in " +
                    "doubt, call.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
        )
        Spacer(Modifier.height(8.dp))
        Text(
            // The version is the cheapest proof that the `.so` in this APK is the one that
            // was built and tested. If it reads wrong, nothing above it can be trusted.
            text = "Core ${coreVersion()} · offline · country ${numbers.country ?: "unknown"}",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
        )
    }
}

/**
 * The country and the number, both correctable.
 *
 * The override is here because every source below `USER_OVERRIDE` is a guess this app made,
 * and the person holding the phone may simply know better — they may have their district's
 * ambulance line written on a card by the door. Nothing in the core outranks that.
 */
@Composable
private fun SettingsDialog(
    core: Prohori,
    current: EmergencyNumbers,
    initialOverride: String,
    onDismiss: () -> Unit,
    onApply: (String?, String?) -> Unit,
) {
    val countries = remember { core.knownCountries() }
    var selected by remember { mutableStateOf(current.country) }
    var typed by remember { mutableStateOf(initialOverride) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Emergency number") },
        text = {
            Column {
                OutlinedTextField(
                    value = typed,
                    onValueChange = { typed = it },
                    label = { Text("Number to call (optional)") },
                    supportingText = { Text("If you know your local number, put it here.") },
                    keyboardOptions =
                        KeyboardOptions(
                            keyboardType = KeyboardType.Phone,
                            imeAction = ImeAction.Done,
                        ),
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(12.dp))
                Text("Country", style = MaterialTheme.typography.titleMedium)
                LazyColumn(modifier = Modifier.heightIn(max = 280.dp)) {
                    items(countries) { choice: CountryChoice ->
                        val chosen = choice.code == selected
                        TextButton(
                            onClick = { selected = choice.code },
                            modifier =
                                Modifier
                                    .fillMaxWidth()
                                    .background(
                                        if (chosen) {
                                            MaterialTheme.colorScheme.surfaceVariant
                                        } else {
                                            MaterialTheme.colorScheme.surface
                                        },
                                    ),
                        ) {
                            Text(
                                // The number is in the row so a user can recognise the
                                // right country by the number they already know.
                                text =
                                    (if (chosen) "✓ " else "") +
                                        "${choice.name} · ${choice.ambulance}",
                                modifier = Modifier.weight(1f),
                                style = MaterialTheme.typography.bodyLarge,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = { onApply(selected, typed.trim().ifBlank { null }) }) {
                Text("Save")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/**
 * A labelled block of text.
 *
 * `emphasis` raises the contrast; it never carries the meaning on its own. Every caller
 * passes a `label` that says in words what the emphasis is trying to convey.
 */
@Composable
private fun Notice(label: String, body: String, emphasis: Boolean) {
    Surface(
        color =
            if (emphasis) {
                MaterialTheme.colorScheme.errorContainer
            } else {
                MaterialTheme.colorScheme.surfaceVariant
            },
        contentColor =
            if (emphasis) {
                MaterialTheme.colorScheme.onErrorContainer
            } else {
                MaterialTheme.colorScheme.onSurfaceVariant
            },
        shape = RoundedCornerShape(12.dp),
        modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp),
    ) {
        Column(Modifier.padding(14.dp)) {
            Text(
                text = label,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(4.dp))
            Text(text = body, style = MaterialTheme.typography.bodyLarge)
        }
    }
}
