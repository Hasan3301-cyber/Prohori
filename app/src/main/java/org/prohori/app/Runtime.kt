package org.prohori.app

import android.content.Context
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.net.Uri
import android.telephony.TelephonyManager
import android.util.Log
import org.prohori.core.Prohori
import java.util.Locale
import org.json.JSONObject

enum class AppMode {
    OFFLINE,
    ONLINE,
    CHAT,
}

data class HospitalContact(
    val telegramChatId: String? = null,
    val hotline: String? = null,
    val smsNumber: String? = null,
) {
    val isEmpty: Boolean
        get() = telegramChatId == null && hotline == null && smsNumber == null
}

/**
 * The loaded Rust core, held for the life of the process.
 *
 * One instance: the corpus is parsed on construction, and re-parsing it per keystroke
 * would be waste with no upside. Construction cannot fail — the corpus is compiled into
 * the library — so there is no error state to hold here and no loading screen to show.
 */
object Core {
    val instance: Prohori by lazy(LazyThreadSafetyMode.SYNCHRONIZED) { Prohori() }
}

/**
 * Where the app's guess about the user's country comes from.
 *
 * Ordered deliberately, and not the order that first comes to mind:
 *
 * 1. **Network country.** Where the phone physically is. A Bangladeshi SIM roaming in
 *    India needs 108, not 999, so the network wins over the SIM.
 * 2. **SIM country.** Home country. Correct whenever the user is at home, which is most
 *    of the time, and available with no signal at all — which matters, because this app
 *    is built for the case where there is no service.
 * 3. **Locale.** A weak signal (a phone can be set to any language anywhere) but better
 *    than nothing, and nothing means falling back to 112.
 *
 * None of these is permission-gated. `getNetworkCountryIso` and `getSimCountryIso` are
 * both readable without `READ_PHONE_STATE`.
 *
 * Whatever this returns is a guess, and the UI says so: the core labels the resulting
 * number `BUILT_IN` rather than `CITY_PACK`, and `confirmedLocal` stays false.
 */
object CountryHint {
    fun detect(context: Context): String? {
        val telephony = context.getSystemService(Context.TELEPHONY_SERVICE) as? TelephonyManager
        val candidates =
            listOf(
                runCatching { telephony?.networkCountryIso }.getOrNull(),
                runCatching { telephony?.simCountryIso }.getOrNull(),
                Locale.getDefault().country,
            )
        return candidates.firstNotNullOfOrNull { code ->
            code?.trim()?.takeIf { it.length == 2 }?.uppercase(Locale.ROOT)
        }
    }
}

/**
 * The two things the user is allowed to correct, stored on the device and nowhere else.
 *
 * No medical text is ever written here. What someone typed while frightened is not
 * something this app keeps.
 */
class Settings(context: Context) {
    private val prefs = context.getSharedPreferences("prohori.settings", Context.MODE_PRIVATE)
    private val secure = SecureStore(context)

    var appMode: AppMode
        get() =
            runCatching { AppMode.valueOf(prefs.getString(KEY_MODE, null).orEmpty()) }
                .getOrDefault(AppMode.OFFLINE)
        set(value) = prefs.edit().putString(KEY_MODE, value.name).apply()

    var onboardingSeen: Boolean
        get() = prefs.getBoolean(KEY_ONBOARDING_SEEN, false)
        set(value) = prefs.edit().putBoolean(KEY_ONBOARDING_SEEN, value).apply()

    var country: String?
        get() = prefs.getString(KEY_COUNTRY, null)
        set(value) = prefs.edit().putString(KEY_COUNTRY, value).apply()

    /** A number the user typed because the one shown was wrong. Trusted above all else. */
    var ambulanceOverride: String?
        get() = prefs.getString(KEY_AMBULANCE, null)?.takeIf { it.isNotBlank() }
        set(value) = prefs.edit().putString(KEY_AMBULANCE, value?.trim()).apply()

    var locationIqApiKey: String?
        get() = secure.get(SECRET_LOCATIONIQ)
        set(value) = secure.put(SECRET_LOCATIONIQ, value?.trim())

    var relayBaseUrl: String?
        get() = secure.get(SECRET_RELAY_URL)
        set(value) = secure.put(SECRET_RELAY_URL, value?.trim()?.removeSuffix("/"))

    var relayDeviceToken: String?
        get() = secure.get(SECRET_RELAY_TOKEN)
        set(value) = secure.put(SECRET_RELAY_TOKEN, value?.trim())

    /** Runtime-only secret: encrypted on this device and never copied into BuildConfig. */
    var telegramBotToken: String?
        get() = secure.get(SECRET_TELEGRAM_TOKEN)
        set(value) = secure.put(SECRET_TELEGRAM_TOKEN, value?.trim())

    /** Telegram projection retained for the parallel alert coordinator. */
    fun hospitalContacts(): Map<String, String> =
        hospitalEndpoints().mapNotNull { (key, value) ->
            value.telegramChatId?.let { key to it }
        }.toMap()

    /**
     * Encrypted facility endpoints. Old releases stored each JSON value as a chat-id string;
     * reading both shapes makes the migration automatic and lossless.
     */
    fun hospitalEndpoints(): Map<String, HospitalContact> {
        val raw = secure.get(SECRET_HOSPITAL_CONTACTS) ?: return emptyMap()
        return runCatching {
            val json = JSONObject(raw)
            buildMap {
                json.keys().forEach { key ->
                    val stored = json.opt(key)
                    val contact =
                        when (stored) {
                            is String -> HospitalContact(telegramChatId = stored.validTelegramOrNull())
                            is JSONObject ->
                                HospitalContact(
                                    telegramChatId = stored.optString("telegram").validTelegramOrNull(),
                                    hotline = stored.optString("hotline").validPhoneOrNull(),
                                    smsNumber = stored.optString("sms").validPhoneOrNull(),
                                )
                            else -> HospitalContact()
                        }
                    if (!contact.isEmpty) put(key, contact)
                }
            }
        }.getOrDefault(emptyMap())
    }

    fun setHospitalContact(facilityKey: String, chatId: String?) {
        val existing = hospitalEndpoints()[facilityKey.trim()] ?: HospitalContact()
        setHospitalContact(facilityKey, existing.copy(telegramChatId = chatId?.trim()?.takeIf { it.isNotEmpty() }))
    }

    fun setHospitalContact(facilityKey: String, contact: HospitalContact?) {
        val key = facilityKey.trim()
        require(key.isNotEmpty()) { "facility key is required" }
        val contacts = hospitalEndpoints().toMutableMap()
        val cleaned =
            HospitalContact(
                telegramChatId = contact?.telegramChatId?.trim()?.takeIf { it.isNotEmpty() },
                hotline = contact?.hotline?.trim()?.takeIf { it.isNotEmpty() },
                smsNumber = contact?.smsNumber?.trim()?.takeIf { it.isNotEmpty() },
            )
        require(cleaned.telegramChatId == null || isValidTelegramChatId(cleaned.telegramChatId)) {
            "Telegram chat id must be a numeric id or an @username"
        }
        require(cleaned.hotline == null || isValidHospitalPhone(cleaned.hotline)) {
            "Hotline must be a valid phone number"
        }
        require(cleaned.smsNumber == null || isValidHospitalPhone(cleaned.smsNumber)) {
            "SMS destination must be a valid phone number"
        }
        if (cleaned.isEmpty) {
            contacts.remove(key)
        } else {
            contacts[key] = cleaned
        }
        val json = JSONObject()
        contacts.toSortedMap().forEach { (name, value) ->
            json.put(
                name,
                JSONObject().apply {
                    value.telegramChatId?.let { put("telegram", it) }
                    value.hotline?.let { put("hotline", it) }
                    value.smsNumber?.let { put("sms", it) }
                },
            )
        }
        secure.put(SECRET_HOSPITAL_CONTACTS, json.toString().takeIf { contacts.isNotEmpty() })
    }

    internal fun encryptedValue(name: String): String? = secure.get(name)

    internal fun putEncryptedValue(name: String, value: String?) = secure.put(name, value)

    private companion object {
        const val KEY_MODE = "app_mode"
        const val KEY_ONBOARDING_SEEN = "onboarding_seen_v1"
        const val KEY_COUNTRY = "country_iso"
        const val KEY_AMBULANCE = "ambulance_override"
        const val SECRET_LOCATIONIQ = "locationiq_api_key"
        const val SECRET_RELAY_URL = "relay_base_url"
        const val SECRET_RELAY_TOKEN = "relay_device_token"
        const val SECRET_TELEGRAM_TOKEN = "telegram_bot_token"
        const val SECRET_HOSPITAL_CONTACTS = "hospital_contacts"
    }
}

private val TELEGRAM_CHAT_ID = Regex("^(?:-?[0-9]{5,20}|@[A-Za-z][A-Za-z0-9_]{4,31})$")
private val HOSPITAL_PHONE = Regex("^\\+?[0-9][0-9 ()-]{4,24}$")

internal fun isValidTelegramChatId(value: String?): Boolean =
    !value.isNullOrBlank() && TELEGRAM_CHAT_ID.matches(value.trim())

internal fun isValidHospitalPhone(value: String?): Boolean =
    !value.isNullOrBlank() && HOSPITAL_PHONE.matches(value.trim()) && value.count(Char::isDigit) in 5..20

private fun String?.validTelegramOrNull(): String? =
    this?.trim()?.takeIf(::isValidTelegramChatId)

private fun String?.validPhoneOrNull(): String? =
    this?.trim()?.takeIf(::isValidHospitalPhone)

internal fun hospitalReadinessSms(
    hospitalName: String,
    specialty: String,
    etaMinutes: Long,
): String =
    "Prohori emergency readiness request for $hospitalName. " +
        "Service needed: ${specialty.replace('_', ' ')}. Estimated arrival: about $etaMinutes minutes. " +
        "Please reply YES if the facility can receive the patient now, or NO if unable. " +
        "No patient name, symptoms, or coordinates are included."

/**
 * Open the dialer with the number filled in.
 *
 * `ACTION_DIAL`, not `ACTION_CALL`: see the comment in `AndroidManifest.xml`. Returns
 * false when no dialer resolved, so the UI can show the number as text instead of leaving
 * the user tapping a button that does nothing.
 */
fun dial(context: Context, number: String): Boolean {
    if (number.isBlank()) return false
    val intent = Intent(Intent.ACTION_DIAL, Uri.parse("tel:$number"))
    return try {
        context.startActivity(intent)
        true
    } catch (error: Exception) {
        // Never crash on the dial path. A visible number the user can key in by hand is a
        // usable fallback; a crash is not.
        Log.w("Prohori", "no dialer available for $number", error)
        false
    }
}

/**
 * Hand the card's text to whatever the user wants to send it with.
 *
 * The text is `FirstAidCard.plainText`, which `prohori_core::render::plain_text` builds with
 * the steps, the sources, and the review status in one block. That is the whole reason this
 * function takes a card's text rather than letting a caller assemble its own: a card that
 * leaves the app without "no clinician has reviewed this card" attached is a card whose
 * reader has no way to weigh it, and the person who receives it over SMS cannot see the
 * banner that was on the screen.
 *
 * Nothing here touches the network and nothing is logged. The corpus is published first-aid
 * guidance, not anything about a patient — see `docs/CONVENTIONS.md` on what this app is
 * allowed to record.
 *
 * Returns false when nothing on the device can send text, so the UI can leave the steps on
 * screen instead of showing a button that does nothing.
 */
fun shareCardText(context: Context, title: String, text: String): Boolean {
    if (text.isBlank()) return false
    val send =
        Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_SUBJECT, title)
            putExtra(Intent.EXTRA_TEXT, text)
        }
    return try {
        context.startActivity(Intent.createChooser(send, null))
        true
    } catch (error: Exception) {
        Log.w("Prohori", "nothing available to share text with", error)
        false
    }
}

/** Build the permission-free intent separately so its safety contract can be device-tested. */
fun hospitalSmsIntent(number: String, body: String): Intent =
    Intent(Intent.ACTION_SENDTO).apply {
        data = Uri.parse("smsto:${Uri.encode(number)}")
        putExtra("sms_body", body)
    }

/** Open the user's SMS app with a structured hospital request; requires no SMS permission. */
fun composeHospitalSms(context: Context, number: String, body: String): Boolean {
    if (number.isBlank() || body.isBlank()) return false
    return try {
        context.startActivity(hospitalSmsIntent(number, body))
        true
    } catch (error: Exception) {
        Log.w("Prohori", "no SMS app available", error)
        false
    }
}

/** Copy an operator script without retaining it in app storage. */
fun copyText(context: Context, label: String, text: String): Boolean {
    if (text.isBlank()) return false
    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
        ?: return false
    clipboard.setPrimaryClip(ClipData.newPlainText(label, text))
    return true
}
