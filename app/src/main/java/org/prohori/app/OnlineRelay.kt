package org.prohori.app

import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap

/**
 * The online hospital-confirmation channel: how an alert leaves this device and how the
 * hospital's answer comes back.
 *
 * # Why a relay, and not a bot token in the APK
 *
 * The app must send the alert itself — no human tapping Send in Telegram — and must receive
 * the YES or NO itself. A `tg://resolve` deep link cannot do the first: it only pre-fills a
 * draft. A bot token compiled into the APK cannot do the second, and that is the part worth
 * spelling out, because it looks like it would work.
 *
 * Telegram permits **one** `getUpdates` consumer per bot; a second one is answered with 409
 * Conflict, and the consumer that wins commits the read offset for everybody. So with a
 * shared token and N installs, one phone drains the queue and a YES meant for case A is
 * delivered to the phone waiting on case B — and lost to A forever. That is not a degraded
 * feature. It is a real hospital confirmation that silently never arrives, which is the exact
 * failure `PLAN.md` §7 exists to prevent.
 *
 * A relay is one consumer by construction. It holds the token, receives every reply, and
 * hands each phone only the case that phone asked about. It also costs nothing
 * organisationally: a Telegram bot cannot message a chat that has not added it, so a hospital
 * has to be onboarded by a coordinating party either way.
 *
 * # What crosses the network
 *
 * Four fields: case id, hospital id, specialty, ETA minutes — plus the body the Rust core
 * composed from those same four. No symptom text, no coordinates, no name. The case id is a
 * SHA-256 prefix. See `prohori_core::confirmation::Confirmation::online_body` and its test
 * `nothing_leaving_the_device_contains_patient_text`.
 *
 * # Silence is not consent
 *
 * Every failure in this file returns [SendOutcome.Refused] or `null`, never an optimistic
 * yes. A timeout, a 500, an unparseable body, a reply that says "maybe" — all of them leave
 * the confirmation Awaiting, and the manual YES/NO buttons stay on screen because the relay
 * is one more thing that can be down.
 */

/** The result of trying to send. Named rather than boolean so a refusal carries its reason. */
sealed interface SendOutcome {
    /** The alert reached the hospital's chat. The confirmation may advance to Awaiting. */
    data object Sent : SendOutcome

    /**
     * Nothing was sent. [reason] is shown to the operator verbatim, including whatever
     * Telegram or the relay said, because "could not send" tells someone standing next to a
     * patient nothing they can act on.
     */
    data class Refused(val reason: String) : SendOutcome
}

/** An explicit inbound answer. There is deliberately no third variant for "probably". */
enum class RelayReply {
    YES,
    NO,
}

/**
 * Everything the transport is allowed to know. Assembled from a verified city pack and the
 * core's confirmation state; a transport never composes its own message or picks its own
 * recipient.
 */
data class HospitalAlert(
    val caseId: String,
    val hospitalId: String,
    /** From the signed pack. The relay checks it against its own registry and may refuse. */
    val telegramChatId: String,
    val specialty: String,
    val etaMinutes: UInt,
    /** `Confirmation::online_body()`. Composed in Rust so the wording is tested. */
    val body: String,
)

interface HospitalAlertTransport {
    /** Shown to the operator, so a drill against a local relay is never mistaken for live. */
    val label: String

    suspend fun send(alert: HospitalAlert): SendOutcome

    /**
     * Ask for this one case. Returns null for pending, for every error, and for any reply
     * that is not an unambiguous yes or no.
     */
    suspend fun poll(caseId: String): RelayReply?
}

/** `PRO-` plus eight hex characters, as produced by `prohori_core::confirmation::case_id`. */
private val CASE_ID_WORD = Regex("^PRO-[0-9A-Fa-f]{8}$")

/**
 * The answering word of the reply, and nothing cleverer.
 *
 * "YES" and "NO" only. Not "y", not "ok", not "ready" — an operator who types something else
 * is a person the device operator can call, and a wrong guess here prints "the hospital is
 * ready" on the strength of a word nobody agreed meant that. The instruction the hospital
 * receives names the two words exactly, so this is not a hard bar to clear.
 *
 * Trailing punctuation is tolerated because "YES." and "Yes!" are the same answer. A leading
 * case id is skipped for the same reason: when a chat has two open alerts the relay asks staff
 * to quote the case id, and "PRO-1A2B3C4D YES" is as natural a way to do that as
 * "YES PRO-1A2B3C4D". A case id is addressing, not an answer, so skipping it guesses at
 * nothing. This matches `strict_answer` in `ecoguardian/alerts/prohori_relay.py`, so a drill
 * driven through the debug transport and a case driven through the relay agree about what an
 * answer is.
 */
internal fun parseReplyText(text: String): RelayReply? {
    val words =
        text.trim()
            .split(' ', '\n', '\t', '\r')
            .filter { it.isNotBlank() }
            .take(2)
            .map { it.trim('.', '!', ',', ';', ':', '"', '\'').uppercase(Locale.ROOT) }
    val answering =
        when {
            words.isEmpty() -> return null
            words.size > 1 && CASE_ID_WORD.matches(words[0]) -> words[1]
            else -> words[0]
        }
    return when (answering) {
        "YES" -> RelayReply.YES
        "NO" -> RelayReply.NO
        else -> null
    }
}

/** Is this a base URL we are willing to put a device token on the wire for? */
internal fun isAcceptableRelayBaseUrl(value: String): Boolean {
    val trimmed = value.trim().removeSuffix("/")
    if (trimmed.isEmpty()) return false
    if (trimmed.startsWith("https://")) return trimmed.length > "https://".length
    // Loopback only, and only because that is how a local relay is tested. Anything else in
    // cleartext would expose the device token and every case id to the network it crosses.
    return trimmed.startsWith("http://10.0.2.2") || trimmed.startsWith("http://localhost")
}

/**
 * Talks to the prohori relay. The only transport a release build can construct.
 *
 * @param baseUrl origin of the relay, no trailing slash required.
 * @param deviceToken sent as `X-Prohori-Device-Token`. Scoped to devices: it must not be
 *   able to reach the relay's `POST /hospital-confirmation`, which is the endpoint that
 *   records a reply. A phone that could reach that endpoint could fake a YES to itself.
 */
class RelayTransport(
    baseUrl: String,
    private val deviceToken: String,
) : HospitalAlertTransport {
    private val root = baseUrl.trim().removeSuffix("/")

    init {
        require(isAcceptableRelayBaseUrl(root)) { "relay base URL must be https:// or loopback" }
        require(deviceToken.isNotBlank()) { "relay device token is required" }
    }

    override val label: String = "relay at ${hostOf(root)}"

    override suspend fun send(alert: HospitalAlert): SendOutcome {
        val payload =
            JSONObject()
                .put("case_id", alert.caseId)
                .put("hospital_id", alert.hospitalId)
                .put("telegram_chat_id", alert.telegramChatId)
                .put("specialty", alert.specialty)
                .put("eta_minutes", alert.etaMinutes.toLong())
                .put("body", alert.body)
        val response = request("POST", "$root/prohori/alert", payload)
        return when {
            response == null -> SendOutcome.Refused("the relay could not be reached")
            response.status in 200..299 -> {
                // The relay says what it did. `sent: false` with a 200 is how it reports a
                // chat mismatch, and treating that as success would leave the operator
                // watching for a reply to a message that was never delivered.
                val body = response.json()
                if (body?.optBoolean("sent", false) == true) {
                    SendOutcome.Sent
                } else {
                    SendOutcome.Refused(
                        body?.optString("detail")?.takeIf { it.isNotBlank() }
                            ?: "the relay accepted the request but did not send it",
                    )
                }
            }
            else ->
                SendOutcome.Refused(
                    response.json()?.optString("detail")?.takeIf { it.isNotBlank() }
                        ?: "the relay refused with HTTP ${response.status}",
                )
        }
    }

    override suspend fun poll(caseId: String): RelayReply? {
        val response = request("GET", "$root/prohori/case/$caseId", null) ?: return null
        if (response.status !in 200..299) return null
        return when (response.json()?.optString("status")?.lowercase(Locale.ROOT)) {
            "yes" -> RelayReply.YES
            "no" -> RelayReply.NO
            // "pending", an unknown value, or a malformed body. All of them mean the same
            // thing to the screen: keep waiting.
            else -> null
        }
    }

    private suspend fun request(method: String, url: String, body: JSONObject?): HttpResponse? =
        httpJson(method, url, body) { connection ->
            connection.setRequestProperty("X-Prohori-Device-Token", deviceToken)
        }
}

/**
 * Talks to api.telegram.org directly for a personal bot dedicated to this phone.
 *
 * A runtime token is encrypted with Android Keystore and never embedded in the APK. This
 * transport remains correct only while one device consumes that bot's updates. Organisations
 * and shared deployments must use [RelayTransport].
 */
class DirectBotTransport(
    private val botToken: String,
) : HospitalAlertTransport {
    /** getUpdates offset. In memory only: an offset is not something to persist. */
    private var offset: Long = 0

    /** A parallel batch can have one case in each hospital chat. */
    private val chatsByCase = ConcurrentHashMap<String, String>()
    private val answersByCase = ConcurrentHashMap<String, RelayReply>()
    private val pollLock = Mutex()

    init {
        require(Regex("^[0-9]{5,15}:[A-Za-z0-9_-]{20,}$").matches(botToken)) {
            "not a Telegram bot token"
        }
    }

    /**
     * The numeric prefix of the token and never the secret half, matching
     * `TelegramProvider.bot_id` in the previous project. This string reaches the screen.
     */
    override val label: String = "personal bot ${botToken.substringBefore(':')} (single device)"

    override suspend fun send(alert: HospitalAlert): SendOutcome {
        val payload =
            JSONObject()
                .put("chat_id", alert.telegramChatId)
                .put("text", alert.body)
        val response =
            httpJson("POST", "https://api.telegram.org/bot$botToken/sendMessage", payload) {}
                ?: return SendOutcome.Refused("api.telegram.org could not be reached")
        val body = response.json()
        if (response.status in 200..299 && body?.optBoolean("ok", false) == true) {
            chatsByCase[alert.caseId] = alert.telegramChatId
            return SendOutcome.Sent
        }
        // Telegram's own words. "chat not found" and "bot was blocked by the user" are
        // different problems with different fixes, and paraphrasing them helps nobody.
        return SendOutcome.Refused(
            body?.optString("description")?.takeIf { it.isNotBlank() }
                ?: "Telegram refused with HTTP ${response.status}",
        )
    }

    override suspend fun poll(caseId: String): RelayReply? = pollLock.withLock {
        answersByCase[caseId]?.let { return@withLock it }
        if (chatsByCase[caseId] == null) return@withLock null
        val response =
            httpJson("GET", "https://api.telegram.org/bot$botToken/getUpdates?offset=$offset&timeout=0", null) {}
                ?: return@withLock null
        val body = response.json() ?: return@withLock null
        if (!body.optBoolean("ok", false)) return@withLock null
        val updates = body.optJSONArray("result") ?: return@withLock null
        for (index in 0 until updates.length()) {
            val update = updates.optJSONObject(index) ?: continue
            offset = maxOf(offset, update.optLong("update_id") + 1)
            val message = update.optJSONObject("message") ?: continue
            val text = message.optString("text")
            val quoted = message.optJSONObject("reply_to_message")?.optString("text").orEmpty()
            val reply = parseReplyText(text) ?: continue
            val candidates =
                chatsByCase.entries.filter { (candidateCase, expectedChat) ->
                    isExpectedChat(message.optJSONObject("chat"), expectedChat) &&
                        when {
                            quoted.isNotEmpty() -> quoted.contains(candidateCase)
                            text.contains(candidateCase, ignoreCase = true) -> true
                            else ->
                                chatsByCase.values.count { it == expectedChat } == 1
                        }
                }
            if (candidates.size == 1) answersByCase[candidates.single().key] = reply
        }
        answersByCase[caseId]
    }

    private fun isExpectedChat(chat: JSONObject?, expected: String): Boolean {
        if (chat == null) return false
        if (expected.startsWith("@")) {
            return chat.optString("username").equals(expected.removePrefix("@"), ignoreCase = true)
        }
        return chat.optString("id") == expected
    }
}

/**
 * Which transport this build can use, and why not, when it cannot.
 *
 * Returning the reason rather than just null is the difference between a screen that explains
 * itself and a button that is mysteriously absent.
 */
object AlertTransports {
    fun resolve(settings: Settings? = null): HospitalAlertTransport? =
        runCatching {
            val runtimeRelay = settings?.relayBaseUrl.orEmpty()
            val runtimeRelayToken = settings?.relayDeviceToken.orEmpty()
            val runtimeBot = settings?.telegramBotToken.orEmpty()
            when {
                runtimeRelay.isNotBlank() && runtimeRelayToken.isNotBlank() ->
                    RelayTransport(runtimeRelay, runtimeRelayToken)
                runtimeBot.isNotBlank() -> DirectBotTransport(runtimeBot)
                BuildConfig.DEBUG && BuildConfig.DEBUG_BOT_TOKEN.isNotBlank() ->
                    DirectBotTransport(BuildConfig.DEBUG_BOT_TOKEN)
                BuildConfig.RELAY_BASE_URL.isNotBlank() && BuildConfig.RELAY_DEVICE_TOKEN.isNotBlank() ->
                    RelayTransport(BuildConfig.RELAY_BASE_URL, BuildConfig.RELAY_DEVICE_TOKEN)
                else -> null
            }
        }.getOrElse { error ->
            Log.w("Prohori", "no usable alert transport: ${error.message}")
            null
        }

    fun unavailableReason(settings: Settings? = null): String =
        when {
            settings?.relayBaseUrl?.isNotBlank() == true && settings.relayDeviceToken.isNullOrBlank() ->
                "A relay address is saved, but its device token is missing."
            settings?.telegramBotToken?.isNotBlank() == true ->
                "The personal bot token is not valid. Use a bot dedicated to this phone."
            BuildConfig.RELAY_BASE_URL.isBlank() ->
                "No relay or personal Telegram bot is configured."
            BuildConfig.RELAY_DEVICE_TOKEN.isBlank() ->
                "This build has a relay address but no device token, so the relay would refuse it."
            else -> "The relay address in this build is not usable (HTTPS is required)."
        }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------
//
// `HttpURLConnection` rather than a client library: the app has no HTTP client to reuse, and
// two requests do not justify adding one to an APK that is mostly model weights. Nothing here
// retries — a retry loop against an emergency endpoint is a way to turn one late alert into
// several, and the operator can press the button again.

internal class HttpResponse(val status: Int, private val text: String) {
    fun json(): JSONObject? = runCatching { JSONObject(text) }.getOrNull()
}

private const val CONNECT_TIMEOUT_MILLIS = 10_000
private const val READ_TIMEOUT_MILLIS = 15_000

private suspend fun httpJson(
    method: String,
    url: String,
    body: JSONObject?,
    configure: (HttpURLConnection) -> Unit,
): HttpResponse? =
    withContext(Dispatchers.IO) {
        var connection: HttpURLConnection? = null
        try {
            connection = (URL(url).openConnection() as HttpURLConnection).apply {
                requestMethod = method
                connectTimeout = CONNECT_TIMEOUT_MILLIS
                readTimeout = READ_TIMEOUT_MILLIS
                setRequestProperty("Accept", "application/json")
                instanceFollowRedirects = false
                configure(this)
            }
            if (body != null) {
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json; charset=utf-8")
                connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
            }
            val status = connection.responseCode
            // A refusal's body is on the error stream, and the refusal's text is the useful
            // part, so both streams are read the same way.
            val stream = if (status in 200..299) connection.inputStream else connection.errorStream
            val text = stream?.bufferedReader(Charsets.UTF_8)?.use { it.readText() }.orEmpty()
            HttpResponse(status, text)
        } catch (error: IOException) {
            // Only scheme/host/path are logged; a personal Telegram token lives in the raw path.
            Log.w("Prohori", "$method ${safeLogUrl(url)} failed: ${error.javaClass.simpleName}")
            null
        } catch (error: SecurityException) {
            Log.w("Prohori", "$method ${safeLogUrl(url)} blocked: ${error.javaClass.simpleName}")
            null
        } finally {
            connection?.disconnect()
        }
    }

private fun hostOf(url: String): String = runCatching { URL(url).host }.getOrNull() ?: url

/** Never put a Telegram token or LocationIQ key into logcat. */
private fun safeLogUrl(url: String): String =
    runCatching {
        val parsed = URL(url)
        "${parsed.protocol}://${parsed.host}${parsed.path.replace(Regex("/bot[^/]+"), "/bot<redacted>")}"
    }.getOrDefault("<invalid-url>")
