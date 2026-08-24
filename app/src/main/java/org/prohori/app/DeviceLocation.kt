package org.prohori.app

import android.annotation.SuppressLint
import android.content.Context
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Build
import android.os.Bundle
import android.os.Looper
import kotlinx.coroutines.suspendCancellableCoroutine
import java.util.concurrent.Executor
import kotlin.coroutines.resume

class DeviceLocation(context: Context) {
    private val manager = context.applicationContext.getSystemService(LocationManager::class.java)

    @SuppressLint("MissingPermission")
    suspend fun current(): GeoPoint? {
        val provider =
            listOf(LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER)
                .firstOrNull { runCatching { manager.isProviderEnabled(it) }.getOrDefault(false) }
                ?: return null
        val recent = runCatching { manager.getLastKnownLocation(provider) }.getOrNull()
        if (recent != null && System.currentTimeMillis() - recent.time <= MAX_LAST_KNOWN_AGE_MILLIS) {
            return recent.pointOrNull()
        }
        return suspendCancellableCoroutine { continuation ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val executor = Executor { command -> command.run() }
                manager.getCurrentLocation(provider, null, executor) { location ->
                    if (continuation.isActive) continuation.resume(location?.pointOrNull())
                }
            } else {
                @Suppress("DEPRECATION")
                val listener =
                    object : LocationListener {
                        override fun onLocationChanged(location: Location) {
                            manager.removeUpdates(this)
                            if (continuation.isActive) continuation.resume(location.pointOrNull())
                        }

                        override fun onProviderDisabled(provider: String) {
                            manager.removeUpdates(this)
                            if (continuation.isActive) continuation.resume(null)
                        }

                        override fun onProviderEnabled(provider: String) = Unit

                        @Deprecated("Deprecated in Android")
                        override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) = Unit
                    }
                @Suppress("DEPRECATION")
                manager.requestSingleUpdate(provider, listener, Looper.getMainLooper())
                continuation.invokeOnCancellation { manager.removeUpdates(listener) }
            }
        }
    }

    private fun Location.pointOrNull(): GeoPoint? =
        runCatching { GeoPoint(latitude, longitude) }.getOrNull()

    private companion object {
        const val MAX_LAST_KNOWN_AGE_MILLIS = 10 * 60 * 1_000L
    }
}
