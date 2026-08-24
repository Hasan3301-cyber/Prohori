package org.prohori.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.prohori.core.OfflineRouteRequest

/** Device smoke test for the signed, bundled P3 demonstration pack and Rust router. */
@RunWith(AndroidJUnit4::class)
class P3CityPackTest {
    @Test
    fun bundledPackVerifiesAndRoutesEntirelyOffline() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val core = Core.instance
        val install = CityPackStore(context).installActiveOrBundled(core)

        assertTrue("the bundled city pack must pass signature and digest verification", install.accepted)
        assertFalse("the demonstration topology must never claim a field check", install.fieldChecked)

        val info = requireNotNull(core.cityPackInfo())
        val route =
            core.offlineRoute(
                OfflineRouteRequest(
                    latitude = 24.363,
                    longitude = 88.628,
                    specialty = "general_emergency",
                    // Use the signed snapshot time so this immutable fixture cannot expire in CI.
                    nowEpochSeconds = info.builtAtEpochSeconds,
                    vehicleWidthMillimetres = 2_400u,
                    vehicleHeightMillimetres = 3_000u,
                    permitFloodedOriginZone = false,
                ),
            )

        assertTrue(route.error ?: "the bundled demonstration route must be accepted", route.accepted)
        assertFalse(route.fieldChecked)
        assertEquals("rmch", route.hospitalId)
        assertEquals(listOf(1_001u, 1_002u, 1_003u, 1_004u), route.edgeIds)
        assertEquals(780uL, route.estimatedSeconds)
        assertTrue(route.conditionSources.isNotEmpty())
        assertTrue(requireNotNull(route.attribution).contains("NOT FIELD CHECKED"))
    }
}
