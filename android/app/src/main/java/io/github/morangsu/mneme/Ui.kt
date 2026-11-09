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
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject

/** 与 Win 端网页一致的深色配色（见 res/values/colors.xml）。 */
object MnemeColors {
    val Bg = Color(0xFF16171A)
    val Panel = Color(0xFF1E2024)
    val PanelAlt = Color(0xFF24272C)
    val Text = Color(0xFFE6E7EA)
    val TextDim = Color(0xFF9A9CA3)
    val Accent = Color(0xFF6AA9FF)
    val Divider = Color(0xFF2C2F35)
    val Danger = Color(0xFFFF8A80)
    val Warn = Color(0xFFFFCC80)
    val Ok = Color(0xFF81C784)
}

private val DarkScheme = darkColorScheme(
    primary = MnemeColors.Accent,
    onPrimary = MnemeColors.Bg,
    secondary = MnemeColors.Accent,
    background = MnemeColors.Bg,
    onBackground = MnemeColors.Text,
    surface = MnemeColors.Panel,
    onSurface = MnemeColors.Text,
    surfaceVariant = MnemeColors.PanelAlt,
    onSurfaceVariant = MnemeColors.TextDim,
    outline = MnemeColors.Divider,
    error = MnemeColors.Danger,
)

@Composable
fun MnemeTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = DarkScheme, content = content)
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
// 共用组件
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
            .background(MnemeColors.Panel, RoundedCornerShape(12.dp))
            .border(1.dp, MnemeColors.Divider, RoundedCornerShape(12.dp))
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (title != null) SectionTitle(title)
        content()
    }
}

@Composable
fun SectionTitle(text: String) {
    Text(text, color = MnemeColors.Text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
}

@Composable
fun Dim(text: String, modifier: Modifier = Modifier) {
    Text(text, color = MnemeColors.TextDim, fontSize = 12.sp, modifier = modifier)
}

@Composable
fun Badge(text: String, color: Color = MnemeColors.Accent) {
    Text(
        text,
        color = color,
        fontSize = 10.sp,
        modifier = Modifier
            .background(color.copy(alpha = 0.14f), RoundedCornerShape(4.dp))
            .border(1.dp, color.copy(alpha = 0.5f), RoundedCornerShape(4.dp))
            .padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

@Composable
fun ErrorBanner(message: String) {
    Text(
        "⚠ $message",
        color = MnemeColors.Danger,
        fontSize = 12.sp,
        modifier = Modifier
            .fillMaxWidth()
            .background(MnemeColors.Danger.copy(alpha = 0.10f), RoundedCornerShape(8.dp))
            .padding(8.dp),
    )
}

@Composable
fun LoadingBox() {
    Row(
        modifier = Modifier.padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        CircularProgressIndicator(
            modifier = Modifier.size(18.dp),
            color = MnemeColors.Accent,
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
            containerColor = MnemeColors.Accent,
            contentColor = MnemeColors.Bg,
            disabledContainerColor = MnemeColors.PanelAlt,
            disabledContentColor = MnemeColors.TextDim,
        ),
    ) {
        Text(text, fontWeight = FontWeight.SemiBold, fontSize = 13.sp)
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
        border = BorderStroke(1.dp, MnemeColors.Divider),
        colors = ButtonDefaults.outlinedButtonColors(
            contentColor = MnemeColors.Accent,
            disabledContentColor = MnemeColors.TextDim,
        ),
    ) {
        Text(text, fontSize = 13.sp)
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
        label = { Text(label, color = MnemeColors.TextDim, fontSize = 12.sp) },
        modifier = modifier.fillMaxWidth(),
        singleLine = singleLine,
        maxLines = maxLines,
        textStyle = LocalTextStyle.current.copy(color = MnemeColors.Text, fontSize = 14.sp),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = MnemeColors.Accent,
            unfocusedBorderColor = MnemeColors.Divider,
            focusedLabelColor = MnemeColors.Accent,
            cursorColor = MnemeColors.Accent,
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
                    color = MnemeColors.TextDim,
                    fontSize = 12.sp,
                    modifier = Modifier.width(104.dp),
                )
                Text(
                    obj.readable(k).ifBlank { "—" },
                    color = MnemeColors.Text,
                    fontSize = 12.sp,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}
