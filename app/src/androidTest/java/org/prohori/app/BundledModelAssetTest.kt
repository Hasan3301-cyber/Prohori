package org.prohori.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class BundledModelAssetTest {
    @Test
    fun verified_q4_model_is_packaged_uncompressed() {
        val assets = InstrumentationRegistry.getInstrumentation().targetContext.assets
        assets.openFd(BUNDLED_MODEL_ASSET).use { descriptor ->
            assertEquals(BUNDLED_MODEL_BYTES, descriptor.length)
        }
        assets.open(BUNDLED_MODEL_ASSET).use { input ->
            val magic = ByteArray(4)
            assertEquals(4, input.read(magic))
            assertArrayEquals(byteArrayOf('G'.code.toByte(), 'G'.code.toByte(), 'U'.code.toByte(), 'F'.code.toByte()), magic)
        }
    }
}
