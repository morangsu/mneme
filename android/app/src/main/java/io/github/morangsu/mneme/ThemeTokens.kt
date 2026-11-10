package io.github.morangsu.mneme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/** 由 tools/gen-theme.mjs 从 design/tokens.json 生成，请勿手改。 */
object MnemeTokens {
    val Primary = Color(0xFF6AA9FF)
    val OnPrimary = Color(0xFF0B1A2B)
    val PrimaryContainer = Color(0xFF1D3A5C)
    val OnPrimaryContainer = Color(0xFFD6E8FF)
    val Secondary = Color(0xFF8FAFD8)
    val OnSecondary = Color(0xFF0B1A2B)
    val SecondaryContainer = Color(0xFF253448)
    val OnSecondaryContainer = Color(0xFFDCE9F7)
    val Tertiary = Color(0xFF81C784)
    val OnTertiary = Color(0xFF0B2A10)
    val Background = Color(0xFF16171A)
    val OnBackground = Color(0xFFE6E7EA)
    val Surface = Color(0xFF16171A)
    val OnSurface = Color(0xFFE6E7EA)
    val SurfaceVariant = Color(0xFF24272C)
    val OnSurfaceVariant = Color(0xFF9A9CA3)
    val SurfaceContainerLowest = Color(0xFF121316)
    val SurfaceContainerLow = Color(0xFF1E2024)
    val SurfaceContainer = Color(0xFF24272C)
    val SurfaceContainerHigh = Color(0xFF2C2F35)
    val SurfaceContainerHighest = Color(0xFF34383F)
    val Outline = Color(0xFF3A3E46)
    val OutlineVariant = Color(0xFF2C2F35)
    val Error = Color(0xFFFF8A80)
    val OnError = Color(0xFF2A0B0B)
    val ErrorContainer = Color(0xFF4A2320)
    val OnErrorContainer = Color(0xFFFFDAD6)
    val Warning = Color(0xFFFFCC80)
    val WarningContainer = Color(0xFF4A3A22)
    val Success = Color(0xFF81C784)
    val SuccessContainer = Color(0xFF1F3A24)
    val Scrim = Color(0xFF000000)
    val InverseSurface = Color(0xFFE6E7EA)
    val InverseOnSurface = Color(0xFF16171A)
    val InversePrimary = Color(0xFF185FA5)

    object Shape {
        val Xs = 4.dp
        val S = 8.dp
        val M = 12.dp
        val L = 16.dp
        val Xl = 28.dp
        val Full = 999.dp
    }

    object Space {
        val Xs = 4.dp
        val S = 8.dp
        val M = 12.dp
        val L = 16.dp
        val Xl = 24.dp
        val Xxl = 32.dp
    }

    object Alpha {
        const val Hover = 0.08f
        const val Focus = 0.12f
        const val Pressed = 0.12f
        const val Selected = 0.14f
        const val DisabledContent = 0.38f
        const val DisabledContainer = 0.12f
    }

    object Type {
        const val DisplaySmallSize = 24
        const val DisplaySmallLine = 32
        const val HeadlineSmallSize = 20
        const val HeadlineSmallLine = 28
        const val TitleLargeSize = 18
        const val TitleLargeLine = 24
        const val TitleMediumSize = 15
        const val TitleMediumLine = 20
        const val BodyLargeSize = 14
        const val BodyLargeLine = 20
        const val BodyMediumSize = 12
        const val BodyMediumLine = 16
        const val LabelLargeSize = 13
        const val LabelLargeLine = 16
        const val LabelSmallSize = 11
        const val LabelSmallLine = 14
    }
}
