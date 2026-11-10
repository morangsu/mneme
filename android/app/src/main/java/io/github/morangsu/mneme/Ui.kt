package io.github.morangsu.mneme

import androidx.compose.foundation.BorderStroke
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
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
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
object MnemeColors {
    val Bg = MnemeTokens.Background
    val Panel = MnemeTokens.SurfaceContainerLow
    val PanelAlt = MnemeTokens.SurfaceContainer
    val Text = MnemeTokens.OnSurface
    val TextDim = MnemeTokens.OnSurfaceVariant
    val Accent = MnemeTokens.Primary
    val Divider = MnemeTokens.OutlineVariant
    val Danger = MnemeTokens.Error
    val Warn = MnemeTokens.Warning
    val Ok = MnemeTokens.Success
}

private val DarkScheme = darkColorScheme(
    primary = MnemeTokens.Primary,
    onPrimary = MnemeTokens.OnPrimary,
    primaryContainer = MnemeTokens.PrimaryContainer,
    onPrimaryContainer = MnemeTokens.OnPrimaryContainer,
    inversePrimary = MnemeTokens.InversePrimary,
    secondary = MnemeTokens.Secondary,
    onSecondary = MnemeTokens.OnSecondary,
    secondaryContainer = MnemeTokens.SecondaryContainer,
    onSecondaryContainer = MnemeTokens.OnSecondaryContainer,
    tertiary = MnemeTokens.Tertiary,
    onTertiary = MnemeTokens.OnTertiary,
    tertiaryContainer = MnemeTokens.SuccessContainer,
    onTertiaryContainer = MnemeTokens.OnSurface,
    background = MnemeTokens.Background,
    onBackground = MnemeTokens.OnBackground,
    surface = MnemeTokens.Surface,
    onSurface = MnemeTokens.OnSurface,
    surfaceVariant = MnemeTokens.SurfaceVariant,
    onSurfaceVariant = MnemeTokens.OnSurfaceVariant,
    surfaceTint = MnemeTokens.Primary,
    inverseSurface = MnemeTokens.InverseSurface,
    inverseOnSurface = MnemeTokens.InverseOnSurface,
    error = MnemeTokens.Error,
    onError = MnemeTokens.OnError,
    errorContainer = MnemeTokens.ErrorContainer,
    onErrorContainer = MnemeTokens.OnErrorContainer,
    outline = MnemeTokens.Outline,
    outlineVariant = MnemeTokens.OutlineVariant,
    scrim = MnemeTokens.Scrim,
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
    MaterialTheme(
        colorScheme = DarkScheme,
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
            .background(MnemeTokens.SurfaceContainerLow, RoundedCornerShape(MnemeTokens.Shape.L))
            .border(1.dp, MnemeTokens.OutlineVariant, RoundedCornerShape(MnemeTokens.Shape.L))
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
        color = MnemeTokens.OnSurface,
        fontSize = MnemeTokens.Type.TitleMediumSize.sp,
        fontWeight = FontWeight.Medium,
    )
}

@Composable
fun Dim(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        color = MnemeTokens.OnSurfaceVariant,
        fontSize = MnemeTokens.Type.BodyMediumSize.sp,
        modifier = modifier,
    )
}

@Composable
fun Badge(text: String, color: Color = MnemeTokens.Primary) {
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
        color = MnemeTokens.OnErrorContainer,
        fontSize = MnemeTokens.Type.BodyMediumSize.sp,
        modifier = Modifier
            .fillMaxWidth()
            .background(MnemeTokens.ErrorContainer, RoundedCornerShape(MnemeTokens.Shape.S))
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
            color = MnemeTokens.Primary,
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
            containerColor = MnemeTokens.Primary,
            contentColor = MnemeTokens.OnPrimary,
            disabledContainerColor = MnemeTokens.SurfaceContainer,
            disabledContentColor = MnemeTokens.OnSurfaceVariant.copy(alpha = MnemeTokens.Alpha.DisabledContent),
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
        border = BorderStroke(1.dp, MnemeTokens.Outline),
        colors = ButtonDefaults.outlinedButtonColors(
            contentColor = MnemeTokens.Primary,
            disabledContentColor = MnemeTokens.OnSurfaceVariant.copy(alpha = MnemeTokens.Alpha.DisabledContent),
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
                color = MnemeTokens.OnSurfaceVariant,
                fontSize = MnemeTokens.Type.BodyMediumSize.sp,
            )
        },
        modifier = modifier.fillMaxWidth(),
        singleLine = singleLine,
        maxLines = maxLines,
        shape = RoundedCornerShape(MnemeTokens.Shape.S),
        textStyle = LocalTextStyle.current.copy(
            color = MnemeTokens.OnSurface,
            fontSize = MnemeTokens.Type.BodyLargeSize.sp,
        ),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = MnemeTokens.Primary,
            unfocusedBorderColor = MnemeTokens.Outline,
            disabledBorderColor = MnemeTokens.OutlineVariant,
            focusedLabelColor = MnemeTokens.Primary,
            unfocusedLabelColor = MnemeTokens.OnSurfaceVariant,
            cursorColor = MnemeTokens.Primary,
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
                    color = MnemeTokens.OnSurfaceVariant,
                    fontSize = MnemeTokens.Type.BodyMediumSize.sp,
                    modifier = Modifier.width(104.dp),
                )
                Text(
                    obj.readable(k).ifBlank { "—" },
                    color = MnemeTokens.OnSurface,
                    fontSize = MnemeTokens.Type.BodyMediumSize.sp,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}
