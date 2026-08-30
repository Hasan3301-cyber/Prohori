package org.prohori.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import java.util.concurrent.atomic.AtomicInteger

class OnlineRoutingTest {
    @Test
    fun nearby_parser_keeps_real_medical_places_and_rejects_noise() {
        val parsed =
            parseNearby(
                """
                [
                  {"place_id":"1","osm_type":"way","osm_id":"11","lat":"24.36","lon":"88.62","type":"clinic","name":"City Clinic","display_name":"City Clinic, Rajshahi","distance":100},
                  {"place_id":"2","osm_type":"node","osm_id":"22","lat":"24.37","lon":"88.63","type":"hospital","name":"General Hospital","display_name":"General Hospital, Rajshahi","distance":800},
                  {"place_id":"3","osm_type":"node","osm_id":"33","lat":"24.38","lon":"88.64","type":"pharmacy","name":"Medicine Store","distance":20},
                  {"place_id":"4","osm_type":"node","osm_id":"44","lat":"NaN","lon":"88.64","type":"hospital","name":"Broken Hospital","distance":30},
                  {"place_id":"5","osm_type":"node","osm_id":"55","lat":"24.38","lon":"88.64","type":"hospital","name":"Blood Bank Annex","distance":30}
                ]
                """.trimIndent(),
            )
        assertEquals(listOf("General Hospital", "City Clinic"), parsed.map { it.name })
        assertEquals("OSM-N22", parsed.first().facilityId)
    }

    @Test
    fun directions_parser_preserves_steps_and_only_claims_explicit_traffic() {
        val route =
            parseDirections(
                """
                {"code":"Ok","metadata":{"datasource_names":["traffic","lua profile"]},"routes":[{
                  "duration":620.4,"distance":4100.2,
                  "legs":[{"steps":[{"distance":300,"name":"Main Road","maneuver":{"type":"turn","modifier":"left","instruction":"Turn left onto Main Road"}}]}]
                }]}
                """.trimIndent(),
                hospital("H1", "A Hospital", 620).hospital,
            )
        assertEquals(620, route.durationSeconds)
        assertTrue(route.trafficSourceReported)
        assertEquals("Turn left onto Main Road", route.steps.single().instruction)
    }

    @Test
    fun matrix_parser_returns_all_six_routes_in_destination_order() {
        val hospitals =
            (1..6).map { index ->
                hospital("H$index", "Hospital $index", 100L * index).hospital
            }
        val routes =
            parseMatrix(
                """
                {"code":"Ok","durations":[[610,420,900,305,730,515]],
                 "distances":[[4100,2800,6500,1900,5000,3500]]}
                """.trimIndent(),
                hospitals,
            )
        assertEquals(6, routes.size)
        assertEquals((1..6).map { "H$it" }, routes.map { it.hospital.facilityId })
        assertEquals(305, routes[3].durationSeconds)
        assertTrue(routes.all { it.steps.isEmpty() && !it.trafficSourceReported })
    }

    @Test
    fun matrix_parser_refuses_to_silently_drop_a_shortlisted_hospital() {
        val hospitals =
            (1..3).map { index ->
                hospital("H$index", "Hospital $index", 100L * index).hospital
            }
        assertThrows(IllegalArgumentException::class.java) {
            parseMatrix(
                """{"code":"Ok","durations":[[610,420]],"distances":[[4100,2800]]}""",
                hospitals,
            )
        }
    }

    @Test
    fun detailed_route_replaces_only_the_confirmed_hospital_summary() {
        val summaries = (1..3).map { hospital("H$it", "Hospital $it", 100L * it) }
        val snapshot = OnlineRouteSnapshot(GeoPoint(24.36, 88.62), 1, summaries)
        val detailed = summaries[1].copy(steps = listOf(OnlineRouteStep("Turn left", 200)))
        val refreshed = snapshot.withDetailedRoute(detailed)
        assertEquals(3, refreshed.routes.size)
        assertTrue(refreshed.routes[0].steps.isEmpty())
        assertEquals("Turn left", refreshed.routes[1].steps.single().instruction)
        assertTrue(refreshed.routes[2].steps.isEmpty())
    }

    @Test
    fun confirmation_parser_never_guesses() {
        assertEquals(RelayReply.YES, parseReplyText("PRO-12AB34CD YES"))
        assertEquals(RelayReply.NO, parseReplyText("No!"))
        assertNull(parseReplyText("ready"))
        assertNull(parseReplyText("maybe yes"))
    }

    @Test
    fun destination_selection_requires_confirmation_then_uses_route_time() {
        val slower = dispatch("H2", 800, HospitalAlertState.CONFIRMED)
        val faster = dispatch("H1", 500, HospitalAlertState.CONFIRMED)
        val unconfirmed = dispatch("H0", 100, HospitalAlertState.AWAITING)
        assertEquals("H1", HospitalCoordinator.bestConfirmed(listOf(slower, unconfirmed, faster))?.route?.hospital?.facilityId)
        assertNull(HospitalCoordinator.bestConfirmed(listOf(unconfirmed)))
    }

    @Test
    fun credentials_and_contacts_have_fail_closed_validation() {
        assertTrue(isValidTelegramChatId("-1001234567890"))
        assertTrue(isValidTelegramChatId("@HospitalEmergency"))
        assertFalse(isValidTelegramChatId("hospital name"))
        assertTrue(isAcceptableRelayBaseUrl("https://relay.example.org"))
        assertFalse(isAcceptableRelayBaseUrl("http://relay.example.org"))
        assertTrue(isValidHospitalPhone("+880 1712-345678"))
        assertTrue(isValidHospitalPhone("99999"))
        assertFalse(isValidHospitalPhone("call hospital now"))
        assertFalse(isValidHospitalPhone("12"))
    }

    @Test
    fun emergency_protocol_maps_to_coarse_service_without_diagnosing() {
        assertEquals("cardiac_emergency", emergencyCareTarget("chest.pain").specialty)
        assertEquals("trauma_emergency", emergencyCareTarget("head.injury").specialty)
        assertEquals("general_emergency", emergencyCareTarget("unknown.protocol").specialty)
    }

    @Test
    fun readiness_sms_contains_only_coarse_operational_data() {
        val body = hospitalReadinessSms("General Hospital", "cardiac_emergency", 8)
        assertTrue(body.contains("cardiac emergency"))
        assertTrue(body.contains("about 8 minutes"))
        assertTrue(body.contains("No patient name, symptoms, or coordinates"))
    }

    @Test
    fun shortlist_is_bounded_and_cached_age_is_explicit() {
        val hospitals =
            (1..9).map { index ->
                OnlineHospital("H$index", "Hospital $index", "Hospital $index", "hospital", GeoPoint(24.36, 88.62), index)
            }
        assertEquals(6, shortlistHospitals(hospitals, 99).size)
        assertEquals("5 min old", cachedRouteAgeLabel(1_000, 301_000))
        assertEquals("2 h old", cachedRouteAgeLabel(1_000, 7_201_000))
    }

    @Test
    fun parallel_coordinator_sends_only_to_registered_facilities() = runBlocking {
        val sent = mutableListOf<String>()
        val transport =
            object : HospitalAlertTransport {
                override val label = "test"
                override suspend fun send(alert: HospitalAlert): SendOutcome {
                    synchronized(sent) { sent += alert.hospitalId }
                    return SendOutcome.Sent
                }
                override suspend fun poll(caseId: String): RelayReply? = null
            }
        val snapshot =
            OnlineRouteSnapshot(
                GeoPoint(24.36, 88.62),
                1,
                listOf(hospital("H1", "One Hospital", 300), hospital("H2", "Two Hospital", 400)),
            )
        val result = HospitalCoordinator(transport).dispatch(snapshot, mapOf("H2" to "@HospitalTwo"))
        assertEquals(listOf("H2"), sent)
        assertEquals(HospitalAlertState.UNREGISTERED, result.first { it.route.hospital.facilityId == "H1" }.state)
        assertEquals(HospitalAlertState.AWAITING, result.first { it.route.hospital.facilityId == "H2" }.state)
    }

    @Test
    fun coordinator_propagates_condition_derived_service_to_every_alert() = runBlocking {
        val specialties = mutableListOf<String>()
        val transport =
            object : HospitalAlertTransport {
                override val label = "test"
                override suspend fun send(alert: HospitalAlert): SendOutcome {
                    synchronized(specialties) { specialties += alert.specialty }
                    return SendOutcome.Sent
                }

                override suspend fun poll(caseId: String): RelayReply? = null
            }
        val snapshot =
            OnlineRouteSnapshot(
                GeoPoint(24.36, 88.62),
                1,
                listOf(hospital("H1", "One Hospital", 300), hospital("H2", "Two Hospital", 400)),
            )

        HospitalCoordinator(transport).dispatch(
            snapshot,
            mapOf("H1" to "@HospitalOne", "H2" to "@HospitalTwo"),
            "trauma_emergency",
        )

        assertEquals(listOf("trauma_emergency", "trauma_emergency"), specialties.sorted())
    }

    @Test
    fun coordinator_fans_out_to_six_registered_hospitals_concurrently() = runBlocking {
        val active = AtomicInteger(0)
        val peak = AtomicInteger(0)
        val sent = mutableSetOf<String>()
        val transport =
            object : HospitalAlertTransport {
                override val label = "test"
                override suspend fun send(alert: HospitalAlert): SendOutcome {
                    val now = active.incrementAndGet()
                    peak.updateAndGet { current -> maxOf(current, now) }
                    delay(60)
                    synchronized(sent) { sent += alert.hospitalId }
                    active.decrementAndGet()
                    return SendOutcome.Sent
                }

                override suspend fun poll(caseId: String): RelayReply? = null
            }
        val routes = (1..6).map { hospital("H$it", "Hospital $it", 200L + it) }
        val contacts = (1..6).associate { "H$it" to "@Hospital$it" }

        val result =
            HospitalCoordinator(transport).dispatch(
                OnlineRouteSnapshot(GeoPoint(24.36, 88.62), 1, routes),
                contacts,
            )

        assertEquals(6, sent.size)
        assertEquals(6, result.count { it.state == HospitalAlertState.AWAITING })
        assertTrue("hospital sends did not overlap", peak.get() > 1)
    }

    @Test
    fun coordinator_never_fans_out_beyond_six_or_duplicates_a_facility() = runBlocking {
        val sent = mutableListOf<String>()
        val transport =
            object : HospitalAlertTransport {
                override val label = "test"
                override suspend fun send(alert: HospitalAlert): SendOutcome {
                    synchronized(sent) { sent += alert.hospitalId }
                    return SendOutcome.Sent
                }
                override suspend fun poll(caseId: String): RelayReply? = null
            }
        val routes = (1..8).map { hospital("H$it", "Hospital $it", 200L + it) } + hospital("H1", "Duplicate", 100)
        val contacts = (1..8).associate { "H$it" to "@Hospital$it" }

        val result = HospitalCoordinator(transport).dispatch(OnlineRouteSnapshot(GeoPoint(24.36, 88.62), 1, routes), contacts)

        assertEquals(6, sent.distinct().size)
        assertEquals(6, result.size)
        assertEquals(sent.distinct().size, sent.size)
    }

    @Test
    fun route_freshness_turns_stale_after_fifteen_minutes() {
        assertFalse(routeIsStale(1_000, 901_000))
        assertTrue(routeIsStale(1_000, 901_001))
    }

    @Test
    fun alert_body_contains_no_patient_text_or_coordinates() {
        val body = HospitalCoordinator.alertBody("PRO-12AB34CD", "General Hospital", "burns", 9u)
        assertTrue(body.contains("Reply YES"))
        assertTrue(body.contains("no patient name, symptoms, or coordinates"))
        assertFalse(body.contains("24.36"))
    }

    private fun hospital(id: String, name: String, seconds: Long): OnlineHospitalRoute =
        OnlineHospitalRoute(
            hospital = OnlineHospital(id, name, name, "hospital", GeoPoint(24.36, 88.62), 500),
            durationSeconds = seconds,
            distanceMetres = 2_000,
            trafficSourceReported = false,
            steps = emptyList(),
        )

    private fun dispatch(id: String, seconds: Long, state: HospitalAlertState): HospitalDispatch =
        HospitalDispatch(hospital(id, "$id Hospital", seconds), "PRO-12AB34CD", "@Hospital$id", state, "test")
}
