package org.prohori.app

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.util.Locale
import kotlin.math.abs

data class GeoPoint(val latitude: Double, val longitude: Double) {
    init {
        require(latitude.isFinite() && latitude in -90.0..90.0)
        require(longitude.isFinite() && longitude in -180.0..180.0)
    }
}

data class OnlineHospital(
    val facilityId: String,
    val name: String,
    val displayName: String,
    val kind: String,
    val location: GeoPoint,
    val straightDistanceMetres: Int,
)

data class OnlineRouteStep(val instruction: String, val distanceMetres: Int)

data class OnlineHospitalRoute(
    val hospital: OnlineHospital,
    val durationSeconds: Long,
    val distanceMetres: Long,
    val trafficSourceReported: Boolean,
    val steps: List<OnlineRouteStep>,
)

data class OnlineRouteSnapshot(
    val origin: GeoPoint,
    val fetchedAtEpochMillis: Long,
    val routes: List<OnlineHospitalRoute>,
)

/** Strict LocationIQ client. Provider failures are errors, never claims that no hospital exists. */
class LocationIqClient(private val apiKey: String) {
    init {
        require(apiKey.isNotBlank()) { "LocationIQ API key is required" }
    }

    suspend fun discoverRoutes(
        origin: GeoPoint,
        radiusMetres: Int = 30_000,
        limit: Int = 6,
    ): OnlineRouteSnapshot = coroutineScope {
        val hospitals =
            parseNearby(
                request(
                    endpoint = "nearby",
                    url = nearbyUrl(origin, radiusMetres.coerceIn(100, 30_000)),
                ),
            ).let { shortlistHospitals(it, limit) }
        require(hospitals.isNotEmpty()) { "LocationIQ returned no usable medical facilities" }

        // Nearby and Routing may share a per-second quota. A single Matrix request
        // calculates all six ETAs without the six-request burst that previously
        // caused five HTTP 429 responses on a valid key.
        delay(PROVIDER_REQUEST_GAP_MILLIS)
        val routes =
            runCatching {
                parseMatrix(request("matrix", matrixUrl(origin, hospitals)), hospitals)
            }.getOrElse {
                // Older/restricted keys may not include Matrix. Preserve the full
                // fan-out with quota-safe individual requests instead of silently
                // shrinking the candidate list to the first successful route.
                routeIndividually(origin, hospitals)
            }.sortedWith(compareBy({ it.durationSeconds }, { it.hospital.facilityId }))
        require(routes.isNotEmpty()) { "LocationIQ returned no usable driving routes" }
        OnlineRouteSnapshot(origin, System.currentTimeMillis(), routes)
    }

    /** Fetch turn-by-turn data only after a hospital explicitly confirms. */
    suspend fun detailedRoute(origin: GeoPoint, hospital: OnlineHospital): OnlineHospitalRoute =
        parseDirections(
            request("directions", directionsUrl(origin, hospital.location)),
            hospital,
        )

    private fun nearbyUrl(origin: GeoPoint, radius: Int): String =
        "$API_ROOT/nearby?key=${encode(apiKey)}" +
            "&lat=${coordinate(origin.latitude)}&lon=${coordinate(origin.longitude)}" +
            "&tag=${encode("amenity:hospital,amenity:clinic,amenity:doctors")}" +
            "&radius=$radius&limit=20&format=json&accept-language=en"

    private fun directionsUrl(origin: GeoPoint, destination: GeoPoint): String =
        "$API_ROOT/directions/driving/" +
            "${coordinate(origin.longitude)},${coordinate(origin.latitude)};" +
            "${coordinate(destination.longitude)},${coordinate(destination.latitude)}" +
            "?key=${encode(apiKey)}&alternatives=true&steps=true&overview=false"

    private fun matrixUrl(origin: GeoPoint, hospitals: List<OnlineHospital>): String {
        require(hospitals.isNotEmpty() && hospitals.size <= 6)
        val coordinates =
            listOf(origin) + hospitals.map { it.location }
        val destinations = (1..hospitals.size).joinToString(";")
        return "$API_ROOT/matrix/driving/" +
            coordinates.joinToString(";") { "${coordinate(it.longitude)},${coordinate(it.latitude)}" } +
            "?key=${encode(apiKey)}&sources=0&destinations=${encode(destinations)}" +
            "&annotations=duration,distance"
    }

    private suspend fun routeIndividually(
        origin: GeoPoint,
        hospitals: List<OnlineHospital>,
    ): List<OnlineHospitalRoute> {
        val routes = mutableListOf<OnlineHospitalRoute>()
        hospitals.forEach { hospital ->
            delay(PROVIDER_REQUEST_GAP_MILLIS)
            runCatching {
                parseDirections(
                    request("directions", directionsUrl(origin, hospital.location)),
                    hospital,
                )
            }.getOrNull()?.let(routes::add)
        }
        require(routes.size == hospitals.size) {
            "LocationIQ could not calculate a route for every shortlisted hospital"
        }
        return routes
    }

    private suspend fun request(endpoint: String, url: String): String {
        var lastRateLimit: LocationIqHttpException? = null
        repeat(MAX_RATE_LIMIT_ATTEMPTS) { attempt ->
            try {
                return withContext(Dispatchers.IO) { get(endpoint, url) }
            } catch (error: LocationIqHttpException) {
                if (error.statusCode != 429) throw error
                lastRateLimit = error
                if (attempt + 1 < MAX_RATE_LIMIT_ATTEMPTS) {
                    delay(error.retryAfterMillis.coerceIn(1_000L, 5_000L))
                }
            }
        }
        throw requireNotNull(lastRateLimit)
    }

    private fun get(endpoint: String, url: String): String {
        val connection = URL(url).openConnection() as HttpURLConnection
        return try {
            connection.requestMethod = "GET"
            connection.connectTimeout = CONNECT_TIMEOUT_MILLIS
            connection.readTimeout = READ_TIMEOUT_MILLIS
            connection.setRequestProperty("Accept", "application/json")
            val status = connection.responseCode
            if (status !in 200..299) {
                val reason =
                    when (status) {
                        401, 403 -> "API key was refused"
                        404 -> "no result"
                        429 -> "rate limit reached"
                        else -> "HTTP $status"
                    }
                val retryAfterMillis =
                    connection.getHeaderField("Retry-After")?.toLongOrNull()?.times(1_000L)
                        ?: PROVIDER_REQUEST_GAP_MILLIS
                throw LocationIqHttpException(
                    statusCode = status,
                    retryAfterMillis = retryAfterMillis,
                    message = "LocationIQ $endpoint failed: $reason",
                )
            }
            connection.inputStream.use { input ->
                val output = ByteArrayOutputStream()
                val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    require(output.size() + read <= MAX_RESPONSE_BYTES) {
                        "LocationIQ $endpoint response was too large"
                    }
                    output.write(buffer, 0, read)
                }
                output.toString(Charsets.UTF_8.name())
            }
        } catch (error: IOException) {
            throw IllegalStateException("LocationIQ $endpoint could not be reached")
        } catch (error: SecurityException) {
            throw IllegalStateException("LocationIQ $endpoint was blocked by this device")
        } finally {
            connection.disconnect()
        }
    }

    private companion object {
        const val API_ROOT = "https://us1.locationiq.com/v1"
        const val CONNECT_TIMEOUT_MILLIS = 8_000
        const val READ_TIMEOUT_MILLIS = 12_000
        const val MAX_RESPONSE_BYTES = 2_000_000
        const val PROVIDER_REQUEST_GAP_MILLIS = 1_100L
        const val MAX_RATE_LIMIT_ATTEMPTS = 3
    }
}

private class LocationIqHttpException(
    val statusCode: Int,
    val retryAfterMillis: Long,
    message: String,
) : IllegalStateException(message)

internal fun shortlistHospitals(hospitals: List<OnlineHospital>, limit: Int): List<OnlineHospital> =
    hospitals.take(limit.coerceIn(1, 6))

internal fun parseNearby(raw: String): List<OnlineHospital> {
    val array = JSONArray(raw)
    val seenIds = mutableSetOf<String>()
    val seenNames = mutableListOf<Pair<String, GeoPoint>>()
    val hospitals = mutableListOf<OnlineHospital>()
    for (index in 0 until array.length()) {
        val item = array.optJSONObject(index) ?: continue
        val kind = item.optString("type", item.optString("tag_type")).lowercase(Locale.ROOT)
        if (kind !in setOf("hospital", "clinic", "doctors")) continue
        val name = item.optString("name").trim().take(120)
        if (name.length < 4 || EXCLUDED_FACILITY_WORDS.any { name.contains(it, ignoreCase = true) }) continue
        val lat = item.optString("lat").toDoubleOrNull() ?: item.optDouble("lat", Double.NaN)
        val lon = item.optString("lon").toDoubleOrNull() ?: item.optDouble("lon", Double.NaN)
        if (!lat.isFinite() || lat !in -90.0..90.0 || !lon.isFinite() || lon !in -180.0..180.0) continue
        val location = GeoPoint(lat, lon)
        val osmType = item.optString("osm_type").take(1).uppercase(Locale.ROOT).ifBlank { "X" }
        val osmId = item.optString("osm_id").trim()
        val placeId = item.optString("place_id").trim()
        val facilityId = if (osmId.isNotEmpty()) "OSM-$osmType$osmId" else "LIQ-$placeId"
        if (facilityId.endsWith("-") || !seenIds.add(facilityId)) continue
        val normalized = name.lowercase(Locale.ROOT).replace(Regex("[^a-z0-9]+"), " ").trim()
        if (seenNames.any { (other, point) -> other == normalized && near(point, location) }) continue
        seenNames += normalized to location
        val distance = item.optDouble("distance", 0.0)
        if (!distance.isFinite() || distance < 0) continue
        hospitals +=
            OnlineHospital(
                facilityId = facilityId,
                name = name,
                displayName = item.optString("display_name", name).trim().take(240),
                kind = kind,
                location = location,
                straightDistanceMetres = distance.toInt(),
            )
    }
    return hospitals.sortedWith(
        compareBy<OnlineHospital>({ FACILITY_RANK[it.kind] ?: 9 }, { it.straightDistanceMetres }, { it.facilityId }),
    )
}

internal fun parseDirections(raw: String, hospital: OnlineHospital): OnlineHospitalRoute {
    val root = JSONObject(raw)
    require(root.optString("code", "Ok") == "Ok") { "LocationIQ could not calculate this route" }
    val routes = root.optJSONArray("routes") ?: error("LocationIQ directions had no routes")
    val route = routes.optJSONObject(0) ?: error("LocationIQ directions had no route")
    val duration = route.optDouble("duration", Double.NaN)
    val distance = route.optDouble("distance", Double.NaN)
    require(duration.isFinite() && duration > 0 && distance.isFinite() && distance > 0) {
        "LocationIQ returned an invalid duration or distance"
    }
    val steps = mutableListOf<OnlineRouteStep>()
    val legs = route.optJSONArray("legs") ?: JSONArray()
    for (legIndex in 0 until legs.length()) {
        val legSteps = legs.optJSONObject(legIndex)?.optJSONArray("steps") ?: continue
        for (stepIndex in 0 until legSteps.length()) {
            val step = legSteps.optJSONObject(stepIndex) ?: continue
            val maneuver = step.optJSONObject("maneuver")
            val instruction =
                maneuver?.optString("instruction")?.trim().orEmpty().ifBlank {
                    listOf(
                        maneuver?.optString("type")?.replace('_', ' ')?.trim().orEmpty(),
                        maneuver?.optString("modifier")?.trim().orEmpty(),
                        step.optString("name").trim().takeIf { it.isNotEmpty() }?.let { "onto $it" }.orEmpty(),
                    ).filter { it.isNotEmpty() }.joinToString(" ").replaceFirstChar(Char::uppercase)
                }
            if (instruction.isNotBlank()) {
                steps += OnlineRouteStep(instruction.take(240), step.optDouble("distance", 0.0).coerceAtLeast(0.0).toInt())
            }
        }
    }
    val traffic =
        root.optJSONObject("metadata")?.optJSONArray("datasource_names")
            ?.strings()?.any { it.contains("traffic", ignoreCase = true) } == true
    return OnlineHospitalRoute(
        hospital = hospital,
        durationSeconds = duration.toLong(),
        distanceMetres = distance.toLong(),
        trafficSourceReported = traffic,
        steps = steps.take(30),
    )
}

/** Parse one-origin-to-many-destinations Matrix output in shortlist order. */
internal fun parseMatrix(raw: String, hospitals: List<OnlineHospital>): List<OnlineHospitalRoute> {
    require(hospitals.isNotEmpty() && hospitals.size <= 6)
    val root = JSONObject(raw)
    require(root.optString("code", "Ok") == "Ok") { "LocationIQ could not calculate the route matrix" }
    val durationRows = root.optJSONArray("durations") ?: error("LocationIQ matrix had no durations")
    val distanceRows = root.optJSONArray("distances") ?: error("LocationIQ matrix had no distances")
    val durations = durationRows.optJSONArray(0) ?: error("LocationIQ matrix had no duration row")
    val distances = distanceRows.optJSONArray(0) ?: error("LocationIQ matrix had no distance row")
    require(durations.length() == hospitals.size && distances.length() == hospitals.size) {
        "LocationIQ matrix did not cover every shortlisted hospital"
    }
    return hospitals.indices.map { index ->
        require(!durations.isNull(index) && !distances.isNull(index)) {
            "LocationIQ matrix found no driving route for ${hospitals[index].facilityId}"
        }
        val duration = durations.optDouble(index, Double.NaN)
        val distance = distances.optDouble(index, Double.NaN)
        require(duration.isFinite() && duration > 0 && distance.isFinite() && distance > 0) {
            "LocationIQ matrix returned an invalid duration or distance"
        }
        OnlineHospitalRoute(
            hospital = hospitals[index],
            durationSeconds = duration.toLong(),
            distanceMetres = distance.toLong(),
            trafficSourceReported = false,
            steps = emptyList(),
        )
    }
}

internal fun OnlineRouteSnapshot.withDetailedRoute(route: OnlineHospitalRoute): OnlineRouteSnapshot =
    copy(
        routes =
            routes.map { existing ->
                if (existing.hospital.facilityId == route.hospital.facilityId) route else existing
            },
    )

class OnlineRouteCache(private val settings: Settings) {
    fun save(snapshot: OnlineRouteSnapshot) =
        settings.putEncryptedValue(CACHE_KEY, snapshot.toJson().toString())

    fun load(): OnlineRouteSnapshot? =
        settings.encryptedValue(CACHE_KEY)?.let { raw ->
            runCatching { snapshotFromJson(JSONObject(raw)) }.getOrNull()
        }

    private companion object {
        const val CACHE_KEY = "online_route_snapshot_v1"
    }
}

private fun OnlineRouteSnapshot.toJson(): JSONObject =
    JSONObject()
        .put("fetched_at", fetchedAtEpochMillis)
        .put("origin_lat", origin.latitude)
        .put("origin_lon", origin.longitude)
        .put(
            "routes",
            JSONArray().apply {
                routes.forEach { route ->
                    put(
                        JSONObject()
                            .put("id", route.hospital.facilityId)
                            .put("name", route.hospital.name)
                            .put("display", route.hospital.displayName)
                            .put("kind", route.hospital.kind)
                            .put("lat", route.hospital.location.latitude)
                            .put("lon", route.hospital.location.longitude)
                            .put("straight_m", route.hospital.straightDistanceMetres)
                            .put("duration_s", route.durationSeconds)
                            .put("distance_m", route.distanceMetres)
                            .put("traffic", route.trafficSourceReported)
                            .put(
                                "steps",
                                JSONArray().apply {
                                    route.steps.forEach { step ->
                                        put(JSONObject().put("text", step.instruction).put("distance_m", step.distanceMetres))
                                    }
                                },
                            ),
                    )
                }
            },
        )

private fun snapshotFromJson(root: JSONObject): OnlineRouteSnapshot {
    val routesJson = root.getJSONArray("routes")
    require(routesJson.length() in 1..6)
    val routes =
        (0 until routesJson.length()).map { index ->
            val item = routesJson.getJSONObject(index)
            val hospital =
                OnlineHospital(
                    facilityId = item.getString("id"),
                    name = item.getString("name"),
                    displayName = item.getString("display"),
                    kind = item.getString("kind"),
                    location = GeoPoint(item.getDouble("lat"), item.getDouble("lon")),
                    straightDistanceMetres = item.getInt("straight_m"),
                )
            val stepsJson = item.optJSONArray("steps") ?: JSONArray()
            val steps =
                (0 until stepsJson.length()).mapNotNull { stepIndex ->
                    stepsJson.optJSONObject(stepIndex)?.let {
                        OnlineRouteStep(it.optString("text").take(240), it.optInt("distance_m").coerceAtLeast(0))
                    }
                }
            OnlineHospitalRoute(
                hospital,
                item.getLong("duration_s"),
                item.getLong("distance_m"),
                item.optBoolean("traffic"),
                steps,
            )
        }
    return OnlineRouteSnapshot(
        origin = GeoPoint(root.getDouble("origin_lat"), root.getDouble("origin_lon")),
        fetchedAtEpochMillis = root.getLong("fetched_at"),
        routes = routes,
    )
}

private fun JSONArray.strings(): List<String> =
    (0 until length()).mapNotNull { optString(it).takeIf(String::isNotBlank) }

private fun near(first: GeoPoint, second: GeoPoint): Boolean =
    abs(first.latitude - second.latitude) < 0.0025 && abs(first.longitude - second.longitude) < 0.0025

private fun encode(value: String): String = URLEncoder.encode(value, Charsets.UTF_8.name())

private fun coordinate(value: Double): String = String.format(Locale.US, "%.6f", value)

private val FACILITY_RANK = mapOf("hospital" to 0, "clinic" to 1, "doctors" to 2)

private val EXCLUDED_FACILITY_WORDS =
    listOf("pharmacy", "medical store", "veterinary", "dental lab", "blood bank", "pathology", "laboratory")
