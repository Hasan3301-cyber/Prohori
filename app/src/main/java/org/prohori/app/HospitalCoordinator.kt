package org.prohori.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import java.security.MessageDigest
import java.util.Locale

enum class HospitalAlertState {
    UNREGISTERED,
    SENDING,
    AWAITING,
    CONFIRMED,
    DECLINED,
    FAILED,
}
data class HospitalDispatch(
    val route: OnlineHospitalRoute,
    val caseId: String,
    val chatId: String?,
    val state: HospitalAlertState,
    val detail: String,
    val sentAtEpochMillis: Long? = null,
    val repliedAtEpochMillis: Long? = null,
)

class HospitalCoordinator(private val transport: HospitalAlertTransport) {
    suspend fun dispatch(
        snapshot: OnlineRouteSnapshot,
        contacts: Map<String, String>,
        specialty: String = "general_emergency",
    ): List<HospitalDispatch> = coroutineScope {
        val batchTime = System.currentTimeMillis()
        snapshot.routes
            .distinctBy { it.hospital.facilityId }
            .sortedWith(compareBy({ it.durationSeconds }, { it.hospital.facilityId }))
            .take(MAX_PARALLEL_HOSPITALS)
            .map { route ->
            async(Dispatchers.IO) {
                val chat = contacts[route.hospital.facilityId]
                if (!isValidTelegramChatId(chat)) {
                    return@async HospitalDispatch(
                        route,
                        caseId(batchTime, route.hospital.facilityId),
                        null,
                        HospitalAlertState.UNREGISTERED,
                        "No verified Telegram chat is registered for this facility.",
                    )
                }
                val caseId = caseId(batchTime, route.hospital.facilityId)
                val etaMinutes = ((route.durationSeconds + 59) / 60).coerceIn(1, UInt.MAX_VALUE.toLong()).toUInt()
                val body = alertBody(caseId, route.hospital.name, specialty, etaMinutes)
                when (
                    val result =
                        transport.send(
                            HospitalAlert(
                                caseId = caseId,
                                hospitalId = route.hospital.facilityId,
                                telegramChatId = requireNotNull(chat),
                                specialty = specialty,
                                etaMinutes = etaMinutes,
                                body = body,
                            ),
                        )
                ) {
                    SendOutcome.Sent ->
                        HospitalDispatch(
                            route,
                            caseId,
                            chat,
                            HospitalAlertState.AWAITING,
                            "Alert delivered. Waiting for an explicit YES or NO.",
                            sentAtEpochMillis = System.currentTimeMillis(),
                        )
                    is SendOutcome.Refused ->
                        HospitalDispatch(
                            route,
                            caseId,
                            chat,
                            HospitalAlertState.FAILED,
                            result.reason.take(240),
                        )
                }
            }
        }.awaitAll()
            .sortedWith(compareBy({ it.route.durationSeconds }, { it.route.hospital.facilityId }))
    }

    suspend fun poll(dispatches: List<HospitalDispatch>): List<HospitalDispatch> = coroutineScope {
        dispatches.map { dispatch ->
            async(Dispatchers.IO) {
                if (dispatch.state != HospitalAlertState.AWAITING) return@async dispatch
                when (transport.poll(dispatch.caseId)) {
                    RelayReply.YES ->
                        dispatch.copy(
                            state = HospitalAlertState.CONFIRMED,
                            detail = "Hospital explicitly replied YES.",
                            repliedAtEpochMillis = System.currentTimeMillis(),
                        )
                    RelayReply.NO ->
                        dispatch.copy(
                            state = HospitalAlertState.DECLINED,
                            detail = "Hospital explicitly replied NO.",
                            repliedAtEpochMillis = System.currentTimeMillis(),
                        )
                    null -> dispatch
                }
            }
        }.awaitAll()
    }

    companion object {
        const val MAX_PARALLEL_HOSPITALS = 6
        /** Confirmation outranks everything; duration and id make the choice auditable. */
        fun bestConfirmed(dispatches: List<HospitalDispatch>): HospitalDispatch? =
            dispatches.filter { it.state == HospitalAlertState.CONFIRMED }
                .minWithOrNull(compareBy({ it.route.durationSeconds }, { it.route.hospital.facilityId }))

        internal fun alertBody(
            caseId: String,
            hospitalName: String,
            specialty: String,
            etaMinutes: UInt,
        ): String =
            """
            [Prohori emergency readiness request]
            Case ID: $caseId
            Facility: $hospitalName
            Emergency service needed: ${specialty.replace('_', ' ')}
            Estimated travel time: about $etaMinutes minutes

            Reply YES if this facility can receive the patient now, or NO if unable.
            This message contains no patient name, symptoms, or coordinates.
            """.trimIndent()

        private fun caseId(time: Long, facilityId: String): String {
            val digest =
                MessageDigest.getInstance("SHA-256")
                    .digest("$time|$facilityId".toByteArray(Charsets.UTF_8))
            val hex = digest.take(4).joinToString("") { "%02X".format(Locale.ROOT, it) }
            return "PRO-$hex"
        }
    }
}
