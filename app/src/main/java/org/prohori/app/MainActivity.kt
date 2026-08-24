package org.prohori.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.core.view.WindowCompat
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The whole app, phase P0.
 *
 * No network, no model, no permissions. The corpus is inside `libprohori_ffi.so` and every
 * decision on screen was taken by the Rust core.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = false
            isAppearanceLightNavigationBars = true
        }
        // Built here rather than inside `setContent`: a composable lambda re-runs, and a
        // fresh Settings instance on every recomposition would make EmergencyScreen's
        // parameters compare unequal every time for no reason.
        val settings = Settings(applicationContext)
        setContent {
            ProhoriTheme {
                AppScreen(core = Core.instance, settings = settings)
            }
        }
    }
}

internal val ProhoriInk = Color(0xFF0A0A0A)
internal val ProhoriPaper = Color(0xFFF8F9F6)
internal val ProhoriCanvas = Color(0xFFF1F3EE)
internal val ProhoriWhite = Color(0xFFFFFFFF)
internal val ProhoriMuted = Color(0xFF6F746C)
internal val ProhoriBorder = Color(0xFFDDE1D8)
internal val ProhoriGold = Color(0xFFC8A96E)
internal val ProhoriRed = Color(0xFFB42318)
internal val ProhoriRedSoft = Color(0xFFFFE7E3)
internal val ProhoriGreen = Color(0xFF176B45)
internal val ProhoriGreenSoft = Color(0xFFE1F2E8)

@Composable
fun ProhoriTheme(content: @Composable () -> Unit) {
    val scheme =
        lightColorScheme(
            primary = ProhoriInk,
            onPrimary = ProhoriWhite,
            primaryContainer = ProhoriInk,
            onPrimaryContainer = ProhoriWhite,
            secondary = ProhoriGold,
            onSecondary = ProhoriInk,
            secondaryContainer = Color(0xFFF4E9D3),
            onSecondaryContainer = ProhoriInk,
            tertiary = ProhoriGreen,
            tertiaryContainer = ProhoriGreenSoft,
            onTertiaryContainer = Color(0xFF0B3C26),
            background = ProhoriCanvas,
            onBackground = ProhoriInk,
            surface = ProhoriPaper,
            onSurface = ProhoriInk,
            surfaceVariant = ProhoriWhite,
            onSurfaceVariant = ProhoriMuted,
            outline = ProhoriBorder,
            error = ProhoriRed,
            errorContainer = ProhoriRedSoft,
            onErrorContainer = Color(0xFF710C08),
        )

    val typography =
        Typography(
            headlineLarge = TextStyle(
                fontFamily = FontFamily.Serif,
                fontSize = 34.sp,
                lineHeight = 39.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = (-0.4).sp,
            ),
            titleLarge = TextStyle(
                fontFamily = FontFamily.Serif,
                fontSize = 29.sp,
                lineHeight = 34.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = (-0.3).sp,
            ),
            titleMedium = TextStyle(fontSize = 19.sp, lineHeight = 25.sp, fontWeight = FontWeight.Bold),
            bodyLarge = TextStyle(fontSize = 18.sp, lineHeight = 26.sp),
            bodyMedium = TextStyle(fontSize = 16.sp, lineHeight = 23.sp),
            bodySmall = TextStyle(fontSize = 13.sp, lineHeight = 18.sp),
            labelLarge = TextStyle(fontSize = 16.sp, fontWeight = FontWeight.Bold),
            labelMedium = TextStyle(
                fontSize = 12.sp,
                lineHeight = 16.sp,
                fontWeight = FontWeight.Bold,
                letterSpacing = 0.9.sp,
            ),
        )

    val shapes =
        Shapes(
            extraSmall = RoundedCornerShape(8.dp),
            small = RoundedCornerShape(12.dp),
            medium = RoundedCornerShape(18.dp),
            large = RoundedCornerShape(26.dp),
            extraLarge = RoundedCornerShape(34.dp),
        )

    MaterialTheme(colorScheme = scheme, typography = typography, shapes = shapes, content = content)
}
