package org.prohori.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.prohori.core.HospitalConfirmationRequest
import org.prohori.core.HospitalConfirmationStatus
import org.prohori.core.HospitalContactChannel
import org.prohori.core.HospitalReply
import org.prohori.core.HospitalReplySource

@RunWith(AndroidJUnit4::class)
class P4HospitalConfirmationTest {
    @Test
    @Suppress("DEPRECATION")
    fun smsUsesAUserVisibleIntentAndNoRestrictedPermission() {
        val intent = hospitalSmsIntent("+8801700000000", "PROHORI PRO-1234: Reply YES or NO.")
        assertEquals(Intent.ACTION_SENDTO, intent.action)
        assertEquals("smsto", intent.data?.scheme)
        assertEquals("+8801700000000", intent.data?.schemeSpecificPart)
        assertEquals("PROHORI PRO-1234: Reply YES or NO.", intent.getStringExtra("sms_body"))

        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val requested =
            context.packageManager
                .getPackageInfo(context.packageName, PackageManager.GET_PERMISSIONS)
                .requestedPermissions
                .orEmpty()
                .toSet()
        assertFalse(Manifest.permission.SEND_SMS in requested)
        assertFalse(Manifest.permission.READ_SMS in requested)
        assertFalse(Manifest.permission.RECEIVE_SMS in requested)
        assertFalse(Manifest.permission.CALL_PHONE in requested)
    }

    /**
     * Online mode's foreground-only permissions, and everything it still does not request.
     *
     * Asserted against the *merged* manifest, so a library that quietly wants background
     * location or a debug overlay that quietly wants SMS fails here rather than in Play review. The
     * comment at the top of `AndroidManifest.xml` makes a promise; this is what keeps it true.
     */
    @Test
    @Suppress("DEPRECATION")
    fun theMergedManifestRequestsInternetAndForegroundLocationOnly() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val requested =
            context.packageManager
                .getPackageInfo(context.packageName, PackageManager.GET_PERMISSIONS)
                .requestedPermissions
                .orEmpty()
                .toSet()
        assertTrue(
            "the online hospital channel needs INTERNET: $requested",
            Manifest.permission.INTERNET in requested,
        )
        assertTrue(Manifest.permission.ACCESS_COARSE_LOCATION in requested)
        assertTrue(Manifest.permission.ACCESS_FINE_LOCATION in requested)
        for (forbidden in
            listOf(
                Manifest.permission.ACCESS_BACKGROUND_LOCATION,
                Manifest.permission.SEND_SMS,
                Manifest.permission.READ_SMS,
                Manifest.permission.RECEIVE_SMS,
                Manifest.permission.CALL_PHONE,
                Manifest.permission.READ_PHONE_STATE,
                Manifest.permission.READ_CONTACTS,
                Manifest.permission.RECORD_AUDIO,
            )) {
            assertFalse("$forbidden must not be requested", forbidden in requested)
        }
    }

    /**
     * No bot token in this artifact.
     *
     * A token in the APK is extractable with `unzip | strings`, and — worse than the leak —
     * cannot receive replies correctly: Telegram allows one `getUpdates` consumer, so a shared
     * token means one phone drains the queue and another phone's YES is lost. `build.gradle.kts`
     * fails a release build that carries one; this asserts the shape of what shipped.
     */
    @Test
    fun noBuildConfigFieldCarriesATelegramBotToken() {
        val botTokenPattern = Regex("""\d{8,12}:[A-Za-z0-9_-]{30,}""")
        for (value in
            listOf(
                BuildConfig.RELAY_BASE_URL,
                BuildConfig.RELAY_DEVICE_TOKEN,
            )) {
            assertFalse(
                "a relay setting looks like a bot token",
                botTokenPattern.containsMatchIn(value),
            )
        }
        if (!BuildConfig.DEBUG) {
            assertEquals(
                "a release build must carry no bot token",
                "",
                BuildConfig.DEBUG_BOT_TOKEN,
            )
        }
        // Whatever is configured, a cleartext relay outside loopback is refused before a
        // device token is ever put on the wire.
        if (BuildConfig.RELAY_BASE_URL.isNotBlank()) {
            assertTrue(
                "relay base URL must be https or loopback: ${BuildConfig.RELAY_BASE_URL}",
                isAcceptableRelayBaseUrl(BuildConfig.RELAY_BASE_URL),
            )
        }
    }

    /** Only "YES" and "NO" are answers. Everything else leaves the request awaiting. */
    @Test
    fun onlyAnUnambiguousWordIsReadAsAnAnswer() {
        assertEquals(RelayReply.YES, parseReplyText("YES"))
        assertEquals(RelayReply.YES, parseReplyText("yes, bed ready"))
        assertEquals(RelayReply.YES, parseReplyText("  Yes. "))
        assertEquals(RelayReply.NO, parseReplyText("NO"))
        assertEquals(RelayReply.NO, parseReplyText("no — theatre is full"))
        // A chat with two open alerts is told to quote the case id. Both orderings are what
        // staff actually type, so refusing either would make that instruction unfollowable.
        assertEquals(RelayReply.YES, parseReplyText("PRO-1A2B3C4D YES"))
        assertEquals(RelayReply.YES, parseReplyText("pro-1a2b3c4d: yes"))
        assertEquals(RelayReply.YES, parseReplyText("YES PRO-1A2B3C4D"))
        for (ambiguous in
            listOf(
                "", "   ", "maybe", "y", "ok", "ready", "yeah", "not now", "YESTERDAY", "👍",
                // A case id on its own addresses a case without answering it.
                "PRO-1A2B3C4D",
                "PRO-1A2B3C4D we will try",
            )) {
            assertNull("\"$ambiguous\" must not be read as an answer", parseReplyText(ambiguous))
        }
    }

    @Test
    fun onlyAnOperatorRecordedYesCrossesTheNativeBoundaryAsReady() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val core = Core.instance
        assertTrue(CityPackStore(context).installActiveOrBundled(core).accepted)
        val now = (System.currentTimeMillis() / 1_000).toULong()
        val request =
            HospitalConfirmationRequest(
                hospitalId = "rmch",
                specialty = "general_emergency",
                etaMinutes = 13u,
                channel = HospitalContactChannel.VOICE,
                createdAtEpochMillis = System.currentTimeMillis().toULong(),
            )

        val draft = core.startHospitalConfirmation(request)
        assertTrue(draft.error, draft.accepted)
        assertEquals(HospitalConfirmationStatus.DRAFT, draft.confirmation?.status)
        assertFalse(requireNotNull(draft.confirmation).explicitReady)

        val awaiting = core.markHospitalContacted(now + 1u)
        assertTrue(awaiting.error, awaiting.accepted)
        assertEquals(HospitalConfirmationStatus.AWAITING, awaiting.confirmation?.status)
        assertFalse(requireNotNull(awaiting.confirmation).explicitReady)

        val yes = core.recordHospitalReply(HospitalReply.YES, now + 2u, "device operator")
        assertTrue(yes.error, yes.accepted)
        assertEquals(HospitalConfirmationStatus.CONFIRMED, yes.confirmation?.status)
        assertTrue(requireNotNull(yes.confirmation).explicitReady)
        assertNull(yes.error)
        // The screen branches on this to decide whether to name a person, so it must be set.
        assertEquals(HospitalReplySource.OPERATOR, requireNotNull(yes.confirmation).replySource)
    }

    /**
     * The bundled pack binds no Telegram chat, and the refusal has to say which endpoint is
     * missing rather than sending an operator hunting for a phone number.
     *
     * A relay reply is also refused here: the active request is on the voice channel, and a
     * server cannot have overheard a phone call. Passing the *correct* case id is deliberate —
     * the case-id check is not what stops this, the channel is.
     */
    @Test
    fun theOnlineChannelIsRefusedWithoutASignedChatAndARelayCannotAnswerACall() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val core = Core.instance
        assertTrue(CityPackStore(context).installActiveOrBundled(core).accepted)
        val now = (System.currentTimeMillis() / 1_000).toULong()

        val online =
            core.startHospitalConfirmation(
                HospitalConfirmationRequest(
                    hospitalId = "rmch",
                    specialty = "general_emergency",
                    etaMinutes = 13u,
                    channel = HospitalContactChannel.ONLINE,
                    createdAtEpochMillis = System.currentTimeMillis().toULong(),
                ),
            )
        assertFalse(online.accepted)
        assertTrue(
            "the refusal must name the online endpoint: ${online.error}",
            online.error.orEmpty().contains("Telegram"),
        )

        val voice =
            core.startHospitalConfirmation(
                HospitalConfirmationRequest(
                    hospitalId = "rmch",
                    specialty = "general_emergency",
                    etaMinutes = 13u,
                    channel = HospitalContactChannel.VOICE,
                    createdAtEpochMillis = System.currentTimeMillis().toULong(),
                ),
            )
        assertTrue(voice.error, voice.accepted)
        val caseId = requireNotNull(voice.confirmation).caseId
        assertTrue(core.markHospitalContacted(now + 1u).accepted)

        val injected = core.ingestOnlineReply(HospitalReply.YES, now + 2u, caseId)
        assertFalse("a relay must not answer for the voice channel", injected.accepted)
        assertEquals(HospitalConfirmationStatus.AWAITING, injected.confirmation?.status)
        assertFalse(requireNotNull(injected.confirmation).explicitReady)

        val wrongCase = core.ingestOnlineReply(HospitalReply.YES, now + 2u, "PRO-DEADBEEF")
        assertFalse(wrongCase.accepted)
        assertFalse(requireNotNull(wrongCase.confirmation).explicitReady)

        assertTrue(core.expireHospitalConfirmation(now + 3u).accepted)
    }
}
