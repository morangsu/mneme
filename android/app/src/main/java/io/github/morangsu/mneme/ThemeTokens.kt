package io.github.morangsu.mneme

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/** 由 tools/gen-theme.mjs 从 design/tokens.json 生成，请勿手改。 */
object MnemeTokens {
    object Dark {
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
    }

    object Light {
        val Primary = Color(0xFF185FA5)
        val OnPrimary = Color(0xFFFFFFFF)
        val PrimaryContainer = Color(0xFFD6E8FF)
        val OnPrimaryContainer = Color(0xFF0B1A2B)
        val Secondary = Color(0xFF3C6087)
        val OnSecondary = Color(0xFFFFFFFF)
        val SecondaryContainer = Color(0xFFDCE9F7)
        val OnSecondaryContainer = Color(0xFF0B1A2B)
        val Tertiary = Color(0xFF1B6B33)
        val OnTertiary = Color(0xFFFFFFFF)
        val Background = Color(0xFFF7F8FA)
        val OnBackground = Color(0xFF1A1C20)
        val Surface = Color(0xFFF7F8FA)
        val OnSurface = Color(0xFF1A1C20)
        val SurfaceVariant = Color(0xFFEDEFF3)
        val OnSurfaceVariant = Color(0xFF5F636B)
        val SurfaceContainerLowest = Color(0xFFFFFFFF)
        val SurfaceContainerLow = Color(0xFFF3F5F8)
        val SurfaceContainer = Color(0xFFEDEFF3)
        val SurfaceContainerHigh = Color(0xFFE5E8EE)
        val SurfaceContainerHighest = Color(0xFFDDE1E8)
        val Outline = Color(0xFF8A9099)
        val OutlineVariant = Color(0xFFDDE1E8)
        val Error = Color(0xFFB3261E)
        val OnError = Color(0xFFFFFFFF)
        val ErrorContainer = Color(0xFFFFDAD6)
        val OnErrorContainer = Color(0xFF410E0B)
        val Warning = Color(0xFF8A5A00)
        val WarningContainer = Color(0xFFFFE0B2)
        val Success = Color(0xFF1B6B33)
        val SuccessContainer = Color(0xFFC8E6C9)
        val Scrim = Color(0xFF000000)
        val InverseSurface = Color(0xFF2C2F35)
        val InverseOnSurface = Color(0xFFF5F6F8)
        val InversePrimary = Color(0xFF6AA9FF)
    }

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

/** 当前主题的动态色板；MnemeColors 与共用组件都读这里，切主题会整体重组。 */
object MnemePalette {
    private val _primary = mutableStateOf(MnemeTokens.Dark.Primary)
    var Primary: Color
        get() = _primary.value
        set(v) { _primary.value = v }
    private val _onPrimary = mutableStateOf(MnemeTokens.Dark.OnPrimary)
    var OnPrimary: Color
        get() = _onPrimary.value
        set(v) { _onPrimary.value = v }
    private val _primaryContainer = mutableStateOf(MnemeTokens.Dark.PrimaryContainer)
    var PrimaryContainer: Color
        get() = _primaryContainer.value
        set(v) { _primaryContainer.value = v }
    private val _onPrimaryContainer = mutableStateOf(MnemeTokens.Dark.OnPrimaryContainer)
    var OnPrimaryContainer: Color
        get() = _onPrimaryContainer.value
        set(v) { _onPrimaryContainer.value = v }
    private val _secondary = mutableStateOf(MnemeTokens.Dark.Secondary)
    var Secondary: Color
        get() = _secondary.value
        set(v) { _secondary.value = v }
    private val _onSecondary = mutableStateOf(MnemeTokens.Dark.OnSecondary)
    var OnSecondary: Color
        get() = _onSecondary.value
        set(v) { _onSecondary.value = v }
    private val _secondaryContainer = mutableStateOf(MnemeTokens.Dark.SecondaryContainer)
    var SecondaryContainer: Color
        get() = _secondaryContainer.value
        set(v) { _secondaryContainer.value = v }
    private val _onSecondaryContainer = mutableStateOf(MnemeTokens.Dark.OnSecondaryContainer)
    var OnSecondaryContainer: Color
        get() = _onSecondaryContainer.value
        set(v) { _onSecondaryContainer.value = v }
    private val _tertiary = mutableStateOf(MnemeTokens.Dark.Tertiary)
    var Tertiary: Color
        get() = _tertiary.value
        set(v) { _tertiary.value = v }
    private val _onTertiary = mutableStateOf(MnemeTokens.Dark.OnTertiary)
    var OnTertiary: Color
        get() = _onTertiary.value
        set(v) { _onTertiary.value = v }
    private val _background = mutableStateOf(MnemeTokens.Dark.Background)
    var Background: Color
        get() = _background.value
        set(v) { _background.value = v }
    private val _onBackground = mutableStateOf(MnemeTokens.Dark.OnBackground)
    var OnBackground: Color
        get() = _onBackground.value
        set(v) { _onBackground.value = v }
    private val _surface = mutableStateOf(MnemeTokens.Dark.Surface)
    var Surface: Color
        get() = _surface.value
        set(v) { _surface.value = v }
    private val _onSurface = mutableStateOf(MnemeTokens.Dark.OnSurface)
    var OnSurface: Color
        get() = _onSurface.value
        set(v) { _onSurface.value = v }
    private val _surfaceVariant = mutableStateOf(MnemeTokens.Dark.SurfaceVariant)
    var SurfaceVariant: Color
        get() = _surfaceVariant.value
        set(v) { _surfaceVariant.value = v }
    private val _onSurfaceVariant = mutableStateOf(MnemeTokens.Dark.OnSurfaceVariant)
    var OnSurfaceVariant: Color
        get() = _onSurfaceVariant.value
        set(v) { _onSurfaceVariant.value = v }
    private val _surfaceContainerLowest = mutableStateOf(MnemeTokens.Dark.SurfaceContainerLowest)
    var SurfaceContainerLowest: Color
        get() = _surfaceContainerLowest.value
        set(v) { _surfaceContainerLowest.value = v }
    private val _surfaceContainerLow = mutableStateOf(MnemeTokens.Dark.SurfaceContainerLow)
    var SurfaceContainerLow: Color
        get() = _surfaceContainerLow.value
        set(v) { _surfaceContainerLow.value = v }
    private val _surfaceContainer = mutableStateOf(MnemeTokens.Dark.SurfaceContainer)
    var SurfaceContainer: Color
        get() = _surfaceContainer.value
        set(v) { _surfaceContainer.value = v }
    private val _surfaceContainerHigh = mutableStateOf(MnemeTokens.Dark.SurfaceContainerHigh)
    var SurfaceContainerHigh: Color
        get() = _surfaceContainerHigh.value
        set(v) { _surfaceContainerHigh.value = v }
    private val _surfaceContainerHighest = mutableStateOf(MnemeTokens.Dark.SurfaceContainerHighest)
    var SurfaceContainerHighest: Color
        get() = _surfaceContainerHighest.value
        set(v) { _surfaceContainerHighest.value = v }
    private val _outline = mutableStateOf(MnemeTokens.Dark.Outline)
    var Outline: Color
        get() = _outline.value
        set(v) { _outline.value = v }
    private val _outlineVariant = mutableStateOf(MnemeTokens.Dark.OutlineVariant)
    var OutlineVariant: Color
        get() = _outlineVariant.value
        set(v) { _outlineVariant.value = v }
    private val _error = mutableStateOf(MnemeTokens.Dark.Error)
    var Error: Color
        get() = _error.value
        set(v) { _error.value = v }
    private val _onError = mutableStateOf(MnemeTokens.Dark.OnError)
    var OnError: Color
        get() = _onError.value
        set(v) { _onError.value = v }
    private val _errorContainer = mutableStateOf(MnemeTokens.Dark.ErrorContainer)
    var ErrorContainer: Color
        get() = _errorContainer.value
        set(v) { _errorContainer.value = v }
    private val _onErrorContainer = mutableStateOf(MnemeTokens.Dark.OnErrorContainer)
    var OnErrorContainer: Color
        get() = _onErrorContainer.value
        set(v) { _onErrorContainer.value = v }
    private val _warning = mutableStateOf(MnemeTokens.Dark.Warning)
    var Warning: Color
        get() = _warning.value
        set(v) { _warning.value = v }
    private val _warningContainer = mutableStateOf(MnemeTokens.Dark.WarningContainer)
    var WarningContainer: Color
        get() = _warningContainer.value
        set(v) { _warningContainer.value = v }
    private val _success = mutableStateOf(MnemeTokens.Dark.Success)
    var Success: Color
        get() = _success.value
        set(v) { _success.value = v }
    private val _successContainer = mutableStateOf(MnemeTokens.Dark.SuccessContainer)
    var SuccessContainer: Color
        get() = _successContainer.value
        set(v) { _successContainer.value = v }
    private val _scrim = mutableStateOf(MnemeTokens.Dark.Scrim)
    var Scrim: Color
        get() = _scrim.value
        set(v) { _scrim.value = v }
    private val _inverseSurface = mutableStateOf(MnemeTokens.Dark.InverseSurface)
    var InverseSurface: Color
        get() = _inverseSurface.value
        set(v) { _inverseSurface.value = v }
    private val _inverseOnSurface = mutableStateOf(MnemeTokens.Dark.InverseOnSurface)
    var InverseOnSurface: Color
        get() = _inverseOnSurface.value
        set(v) { _inverseOnSurface.value = v }
    private val _inversePrimary = mutableStateOf(MnemeTokens.Dark.InversePrimary)
    var InversePrimary: Color
        get() = _inversePrimary.value
        set(v) { _inversePrimary.value = v }

    fun apply(light: Boolean) {
        if (light) {
            Primary = MnemeTokens.Light.Primary
            OnPrimary = MnemeTokens.Light.OnPrimary
            PrimaryContainer = MnemeTokens.Light.PrimaryContainer
            OnPrimaryContainer = MnemeTokens.Light.OnPrimaryContainer
            Secondary = MnemeTokens.Light.Secondary
            OnSecondary = MnemeTokens.Light.OnSecondary
            SecondaryContainer = MnemeTokens.Light.SecondaryContainer
            OnSecondaryContainer = MnemeTokens.Light.OnSecondaryContainer
            Tertiary = MnemeTokens.Light.Tertiary
            OnTertiary = MnemeTokens.Light.OnTertiary
            Background = MnemeTokens.Light.Background
            OnBackground = MnemeTokens.Light.OnBackground
            Surface = MnemeTokens.Light.Surface
            OnSurface = MnemeTokens.Light.OnSurface
            SurfaceVariant = MnemeTokens.Light.SurfaceVariant
            OnSurfaceVariant = MnemeTokens.Light.OnSurfaceVariant
            SurfaceContainerLowest = MnemeTokens.Light.SurfaceContainerLowest
            SurfaceContainerLow = MnemeTokens.Light.SurfaceContainerLow
            SurfaceContainer = MnemeTokens.Light.SurfaceContainer
            SurfaceContainerHigh = MnemeTokens.Light.SurfaceContainerHigh
            SurfaceContainerHighest = MnemeTokens.Light.SurfaceContainerHighest
            Outline = MnemeTokens.Light.Outline
            OutlineVariant = MnemeTokens.Light.OutlineVariant
            Error = MnemeTokens.Light.Error
            OnError = MnemeTokens.Light.OnError
            ErrorContainer = MnemeTokens.Light.ErrorContainer
            OnErrorContainer = MnemeTokens.Light.OnErrorContainer
            Warning = MnemeTokens.Light.Warning
            WarningContainer = MnemeTokens.Light.WarningContainer
            Success = MnemeTokens.Light.Success
            SuccessContainer = MnemeTokens.Light.SuccessContainer
            Scrim = MnemeTokens.Light.Scrim
            InverseSurface = MnemeTokens.Light.InverseSurface
            InverseOnSurface = MnemeTokens.Light.InverseOnSurface
            InversePrimary = MnemeTokens.Light.InversePrimary
        } else {
            Primary = MnemeTokens.Dark.Primary
            OnPrimary = MnemeTokens.Dark.OnPrimary
            PrimaryContainer = MnemeTokens.Dark.PrimaryContainer
            OnPrimaryContainer = MnemeTokens.Dark.OnPrimaryContainer
            Secondary = MnemeTokens.Dark.Secondary
            OnSecondary = MnemeTokens.Dark.OnSecondary
            SecondaryContainer = MnemeTokens.Dark.SecondaryContainer
            OnSecondaryContainer = MnemeTokens.Dark.OnSecondaryContainer
            Tertiary = MnemeTokens.Dark.Tertiary
            OnTertiary = MnemeTokens.Dark.OnTertiary
            Background = MnemeTokens.Dark.Background
            OnBackground = MnemeTokens.Dark.OnBackground
            Surface = MnemeTokens.Dark.Surface
            OnSurface = MnemeTokens.Dark.OnSurface
            SurfaceVariant = MnemeTokens.Dark.SurfaceVariant
            OnSurfaceVariant = MnemeTokens.Dark.OnSurfaceVariant
            SurfaceContainerLowest = MnemeTokens.Dark.SurfaceContainerLowest
            SurfaceContainerLow = MnemeTokens.Dark.SurfaceContainerLow
            SurfaceContainer = MnemeTokens.Dark.SurfaceContainer
            SurfaceContainerHigh = MnemeTokens.Dark.SurfaceContainerHigh
            SurfaceContainerHighest = MnemeTokens.Dark.SurfaceContainerHighest
            Outline = MnemeTokens.Dark.Outline
            OutlineVariant = MnemeTokens.Dark.OutlineVariant
            Error = MnemeTokens.Dark.Error
            OnError = MnemeTokens.Dark.OnError
            ErrorContainer = MnemeTokens.Dark.ErrorContainer
            OnErrorContainer = MnemeTokens.Dark.OnErrorContainer
            Warning = MnemeTokens.Dark.Warning
            WarningContainer = MnemeTokens.Dark.WarningContainer
            Success = MnemeTokens.Dark.Success
            SuccessContainer = MnemeTokens.Dark.SuccessContainer
            Scrim = MnemeTokens.Dark.Scrim
            InverseSurface = MnemeTokens.Dark.InverseSurface
            InverseOnSurface = MnemeTokens.Dark.InverseOnSurface
            InversePrimary = MnemeTokens.Dark.InversePrimary
        }
    }
}
