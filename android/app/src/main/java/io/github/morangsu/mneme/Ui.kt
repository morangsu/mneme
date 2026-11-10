package io.github.morangsu.mneme

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.core.view.WindowCompat
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Shapes
import androidx.compose.material3.Text
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject

/**
 * 旧色名别名。真值在 ThemeTokens.kt（由 design/tokens.json 生成），
 * 这里保留旧名是为了让 14 个屏幕文件不用逐个改、也不会漏掉某一处硬编码。
 * 新代码请直接用 MnemeTokens 的 Material 3 语义槽位。
 */
/**
 * 旧色名别名。真值在 ThemeTokens.kt（由 design/tokens.json 生成）；
 * 用 getter 委托到动态色板，切亮/暗主题时 14 个屏幕文件不必改一行。
 */
object MnemeColors {
    val Bg get() = MnemePalette.Background
    val Panel get() = MnemePalette.SurfaceContainerLow
    val PanelAlt get() = MnemePalette.SurfaceContainer
    val Text get() = MnemePalette.OnSurface
    val TextDim get() = MnemePalette.OnSurfaceVariant
    val Accent get() = MnemePalette.Primary
    val Divider get() = MnemePalette.OutlineVariant
    val Danger get() = MnemePalette.Error
    val Warn get() = MnemePalette.Warning
    val Ok get() = MnemePalette.Success
}

/** 外观偏好：system / light / dark。改动会触发全局重组。 */
object MnemeAppearance {
    private val state = mutableStateOf("system")
    var mode: String
        get() = state.value
        set(v) { state.value = v }
}

private val DarkScheme = darkColorScheme(
    primary = MnemeTokens.Dark.Primary,
    onPrimary = MnemeTokens.Dark.OnPrimary,
    primaryContainer = MnemeTokens.Dark.PrimaryContainer,
    onPrimaryContainer = MnemeTokens.Dark.OnPrimaryContainer,
    inversePrimary = MnemeTokens.Dark.InversePrimary,
    secondary = MnemeTokens.Dark.Secondary,
    onSecondary = MnemeTokens.Dark.OnSecondary,
    secondaryContainer = MnemeTokens.Dark.SecondaryContainer,
    onSecondaryContainer = MnemeTokens.Dark.OnSecondaryContainer,
    tertiary = MnemeTokens.Dark.Tertiary,
    onTertiary = MnemeTokens.Dark.OnTertiary,
    tertiaryContainer = MnemeTokens.Dark.SuccessContainer,
    onTertiaryContainer = MnemeTokens.Dark.OnSurface,
    background = MnemeTokens.Dark.Background,
    onBackground = MnemeTokens.Dark.OnBackground,
    surface = MnemeTokens.Dark.Surface,
    onSurface = MnemeTokens.Dark.OnSurface,
    surfaceVariant = MnemeTokens.Dark.SurfaceVariant,
    onSurfaceVariant = MnemeTokens.Dark.OnSurfaceVariant,
    surfaceTint = MnemeTokens.Dark.Primary,
    inverseSurface = MnemeTokens.Dark.InverseSurface,
    inverseOnSurface = MnemeTokens.Dark.InverseOnSurface,
    error = MnemeTokens.Dark.Error,
    onError = MnemeTokens.Dark.OnError,
    errorContainer = MnemeTokens.Dark.ErrorContainer,
    onErrorContainer = MnemeTokens.Dark.OnErrorContainer,
    outline = MnemeTokens.Dark.Outline,
    outlineVariant = MnemeTokens.Dark.OutlineVariant,
    scrim = MnemeTokens.Dark.Scrim,
)

private val LightScheme = lightColorScheme(
    primary = MnemeTokens.Light.Primary,
    onPrimary = MnemeTokens.Light.OnPrimary,
    primaryContainer = MnemeTokens.Light.PrimaryContainer,
    onPrimaryContainer = MnemeTokens.Light.OnPrimaryContainer,
    inversePrimary = MnemeTokens.Light.InversePrimary,
    secondary = MnemeTokens.Light.Secondary,
    onSecondary = MnemeTokens.Light.OnSecondary,
    secondaryContainer = MnemeTokens.Light.SecondaryContainer,
    onSecondaryContainer = MnemeTokens.Light.OnSecondaryContainer,
    tertiary = MnemeTokens.Light.Tertiary,
    onTertiary = MnemeTokens.Light.OnTertiary,
    tertiaryContainer = MnemeTokens.Light.SuccessContainer,
    onTertiaryContainer = MnemeTokens.Light.OnSurface,
    background = MnemeTokens.Light.Background,
    onBackground = MnemeTokens.Light.OnBackground,
    surface = MnemeTokens.Light.Surface,
    onSurface = MnemeTokens.Light.OnSurface,
    surfaceVariant = MnemeTokens.Light.SurfaceVariant,
    onSurfaceVariant = MnemeTokens.Light.OnSurfaceVariant,
    surfaceTint = MnemeTokens.Light.Primary,
    inverseSurface = MnemeTokens.Light.InverseSurface,
    inverseOnSurface = MnemeTokens.Light.InverseOnSurface,
    error = MnemeTokens.Light.Error,
    onError = MnemeTokens.Light.OnError,
    errorContainer = MnemeTokens.Light.ErrorContainer,
    onErrorContainer = MnemeTokens.Light.OnErrorContainer,
    outline = MnemeTokens.Light.Outline,
    outlineVariant = MnemeTokens.Light.OutlineVariant,
    scrim = MnemeTokens.Light.Scrim,
)

/** 字阶来自 tokens：与桌面 CSS 的 --type-* 一一对应。 */
private val MnemeTypography = Typography(
    displaySmall = TextStyle(
        fontSize = MnemeTokens.Type.DisplaySmallSize.sp,
        lineHeight = MnemeTokens.Type.DisplaySmallLine.sp,
        fontWeight = FontWeight.Normal,
    ),
    headlineSmall = TextStyle(
        fontSize = MnemeTokens.Type.HeadlineSmallSize.sp,
        lineHeight = MnemeTokens.Type.HeadlineSmallLine.sp,
        fontWeight = FontWeight.Normal,
    ),
    titleLarge = TextStyle(
        fontSize = MnemeTokens.Type.TitleLargeSize.sp,
        lineHeight = MnemeTokens.Type.TitleLargeLine.sp,
        fontWeight = FontWeight.Medium,
    ),
    titleMedium = TextStyle(
        fontSize = MnemeTokens.Type.TitleMediumSize.sp,
        lineHeight = MnemeTokens.Type.TitleMediumLine.sp,
        fontWeight = FontWeight.Medium,
    ),
    bodyLarge = TextStyle(
        fontSize = MnemeTokens.Type.BodyLargeSize.sp,
        lineHeight = MnemeTokens.Type.BodyLargeLine.sp,
        fontWeight = FontWeight.Normal,
    ),
    bodyMedium = TextStyle(
        fontSize = MnemeTokens.Type.BodyMediumSize.sp,
        lineHeight = MnemeTokens.Type.BodyMediumLine.sp,
        fontWeight = FontWeight.Normal,
    ),
    labelLarge = TextStyle(
        fontSize = MnemeTokens.Type.LabelLargeSize.sp,
        lineHeight = MnemeTokens.Type.LabelLargeLine.sp,
        fontWeight = FontWeight.Medium,
    ),
    labelSmall = TextStyle(
        fontSize = MnemeTokens.Type.LabelSmallSize.sp,
        lineHeight = MnemeTokens.Type.LabelSmallLine.sp,
        fontWeight = FontWeight.Medium,
    ),
)

private val MnemeShapes = Shapes(
    extraSmall = RoundedCornerShape(MnemeTokens.Shape.Xs),
    small = RoundedCornerShape(MnemeTokens.Shape.S),
    medium = RoundedCornerShape(MnemeTokens.Shape.M),
    large = RoundedCornerShape(MnemeTokens.Shape.L),
    extraLarge = RoundedCornerShape(MnemeTokens.Shape.Xl),
)

@Composable
fun MnemeTheme(content: @Composable () -> Unit) {
    val dark = when (MnemeAppearance.mode) {
        "light" -> false
        "dark" -> true
        else -> isSystemInDarkTheme()
    }
    // 同步动态别名：值相同时 mutableStateOf 不会触发重组，切主题时才会。
    MnemePalette.apply(!dark)
    // 状态栏跟着主题走：theme.xml 只能给静态启动底色，运行中切换要靠这里。
    val view = LocalView.current
    val ctx = LocalContext.current
    SideEffect {
        var c: android.content.Context? = ctx
        while (c is ContextWrapper && c !is Activity) c = c.baseContext
        val window = (c as? Activity)?.window
        if (window != null) {
            window.statusBarColor = MnemePalette.Background.toArgb()
            WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars = !dark
        }
    }
    MaterialTheme(
        colorScheme = if (dark) DarkScheme else LightScheme,
        typography = MnemeTypography,
        shapes = MnemeShapes,
        content = content,
    )
}

// ---------------------------------------------------------------------------
// JSON 小工具：Rust 侧一律返回字符串，UI 侧统一按 org.json 解析（零额外依赖）
// ---------------------------------------------------------------------------

/** 统一错误字段：所有 FFI 方法出错时约定返回 {"error": "..."}。 */
fun JSONObject.err(): String? = optString("error", "").takeIf { it.isNotBlank() }

/** 把任意类型的值读成可显示文本；数组用 " / " 连接，布尔转「是/否」。 */
fun JSONObject.readable(key: String): String {
    if (!has(key) || isNull(key)) return ""
    return when (val v = get(key)) {
        is JSONArray -> (0 until v.length()).joinToString(" / ") { i -> v.optString(i, "") }
        is JSONObject -> v.toString()
        is Boolean -> if (v) "是" else "否"
        else -> v.toString()
    }
}

/** 依次尝试多个键名，取第一个数组；都不存在时返回空数组（兼容 Rust/HTTP 两种命名）。 */
fun JSONObject.arrayOrEmpty(vararg keys: String): JSONArray {
    for (k in keys) {
        val a = optJSONArray(k)
        if (a != null) return a
    }
    return JSONArray()
}

// ---------------------------------------------------------------------------
// 共用组件：一律走 Material 3 语义槽位，状态层由 MaterialTheme 提供
// ---------------------------------------------------------------------------

@Composable
fun Panel(
    modifier: Modifier = Modifier,
    title: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(
        modifier = modifier
            .fillMaxWidth()
            .background(MnemePalette.SurfaceContainerLow, RoundedCornerShape(MnemeTokens.Shape.L))
            .border(1.dp, MnemePalette.OutlineVariant, RoundedCornerShape(MnemeTokens.Shape.L))
            .padding(MnemeTokens.Space.M),
        verticalArrangement = Arrangement.spacedBy(MnemeTokens.Space.S),
    ) {
        if (title != null) SectionTitle(title)
        content()
    }
}

@Composable
fun SectionTitle(text: String) {
    Text(
        text,
        color = MnemePalette.OnSurface,
        fontSize = MnemeTokens.Type.TitleMediumSize.sp,
        fontWeight = FontWeight.Medium,
    )
}

@Composable
fun Dim(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        color = MnemePalette.OnSurfaceVariant,
        fontSize = MnemeTokens.Type.BodyMediumSize.sp,
        modifier = modifier,
    )
}

@Composable
fun Badge(text: String, color: Color = MnemePalette.Primary) {
    Text(
        text,
        color = color,
        fontSize = MnemeTokens.Type.LabelSmallSize.sp,
        modifier = Modifier
            .background(color.copy(alpha = MnemeTokens.Alpha.Selected), RoundedCornerShape(MnemeTokens.Shape.Xs))
            .border(1.dp, color.copy(alpha = 0.5f), RoundedCornerShape(MnemeTokens.Shape.Xs))
            .padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

@Composable
fun ErrorBanner(message: String) {
    Text(
        "⚠ $message",
        color = MnemePalette.OnErrorContainer,
        fontSize = MnemeTokens.Type.BodyMediumSize.sp,
        modifier = Modifier
            .fillMaxWidth()
            .background(MnemePalette.ErrorContainer, RoundedCornerShape(MnemeTokens.Shape.S))
            .padding(MnemeTokens.Space.S),
    )
}

@Composable
fun LoadingBox() {
    Row(
        modifier = Modifier.padding(MnemeTokens.Space.M),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        CircularProgressIndicator(
            modifier = Modifier.size(18.dp),
            color = MnemePalette.Primary,
            strokeWidth = 2.dp,
        )
        Dim("加载中…")
    }
}

@Composable
fun AccentButton(
    text: String,
    enabled: Boolean = true,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier,
        colors = ButtonDefaults.buttonColors(
            containerColor = MnemePalette.Primary,
            contentColor = MnemePalette.OnPrimary,
            disabledContainerColor = MnemePalette.SurfaceContainer,
            disabledContentColor = MnemePalette.OnSurfaceVariant.copy(alpha = MnemeTokens.Alpha.DisabledContent),
        ),
    ) {
        Text(text, fontWeight = FontWeight.Medium, fontSize = MnemeTokens.Type.LabelLargeSize.sp)
    }
}

@Composable
fun GhostButton(
    text: String,
    enabled: Boolean = true,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    OutlinedButton(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier,
        border = BorderStroke(1.dp, MnemePalette.Outline),
        colors = ButtonDefaults.outlinedButtonColors(
            contentColor = MnemePalette.Primary,
            disabledContentColor = MnemePalette.OnSurfaceVariant.copy(alpha = MnemeTokens.Alpha.DisabledContent),
        ),
    ) {
        Text(text, fontSize = MnemeTokens.Type.LabelLargeSize.sp)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MnemeTextField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    singleLine: Boolean = false,
    maxLines: Int = if (singleLine) 1 else 6,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        label = {
            Text(
                label,
                color = MnemePalette.OnSurfaceVariant,
                fontSize = MnemeTokens.Type.BodyMediumSize.sp,
            )
        },
        modifier = modifier.fillMaxWidth(),
        singleLine = singleLine,
        maxLines = maxLines,
        shape = RoundedCornerShape(MnemeTokens.Shape.S),
        textStyle = LocalTextStyle.current.copy(
            color = MnemePalette.OnSurface,
            fontSize = MnemeTokens.Type.BodyLargeSize.sp,
        ),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = MnemePalette.Primary,
            unfocusedBorderColor = MnemePalette.Outline,
            disabledBorderColor = MnemePalette.OutlineVariant,
            focusedLabelColor = MnemePalette.Primary,
            unfocusedLabelColor = MnemePalette.OnSurfaceVariant,
            cursorColor = MnemePalette.Primary,
            focusedContainerColor = Color.Transparent,
            unfocusedContainerColor = Color.Transparent,
        ),
    )
}

/** 把回执 JSON 的所有顶层键按「键 : 值」列出，便于展示同步/导入/解析结果。 */
@Composable
fun Receipt(obj: JSONObject, skipKeys: Set<String> = setOf("error")) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        val keys = obj.keys()
        while (keys.hasNext()) {
            val k = keys.next()
            if (k in skipKeys) continue
            Row(modifier = Modifier.fillMaxWidth()) {
                Text(
                    k,
                    color = MnemePalette.OnSurfaceVariant,
                    fontSize = MnemeTokens.Type.BodyMediumSize.sp,
                    modifier = Modifier.width(104.dp),
                )
                Text(
                    obj.readable(k).ifBlank { "—" },
                    color = MnemePalette.OnSurface,
                    fontSize = MnemeTokens.Type.BodyMediumSize.sp,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** 外观设置：跟随系统 / 亮色 / 暗色，偏好写本机 SharedPreferences。 */
@Composable
fun AppearancePanel() {
    val ctx = LocalContext.current
    val sp = remember { ctx.getSharedPreferences("mneme_ui", Context.MODE_PRIVATE) }
    Panel(title = "外观") {
        Dim("切换即时生效，并记在本机；跟随系统会读 Android 的深色模式设置。")
        Row(horizontalArrangement = Arrangement.spacedBy(MnemeTokens.Space.S)) {
            listOf("system" to "跟随系统", "light" to "亮色", "dark" to "暗色").forEach { (value, label) ->
                if (MnemeAppearance.mode == value) {
                    AccentButton(label, onClick = {})
                } else {
                    GhostButton(label, onClick = {
                        MnemeAppearance.mode = value
                        sp.edit().putString("theme", value).apply()
                    })
                }
            }
        }
    }
}
