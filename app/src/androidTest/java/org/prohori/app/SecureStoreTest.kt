package org.prohori.app

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SecureStoreTest {
    @Test
    fun secret_round_trips_without_plaintext_in_preferences() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val store = SecureStore(context)
        val name = "instrumentation-secret"
        val secret = "location-token-that-must-not-be-plaintext"
        store.put(name, secret)
        assertEquals(secret, store.get(name))
        val raw = context.getSharedPreferences("prohori.secure", android.content.Context.MODE_PRIVATE)
            .getString(name, "")
            .orEmpty()
        assertFalse(raw.contains(secret))
        store.put(name, null)
    }
}
