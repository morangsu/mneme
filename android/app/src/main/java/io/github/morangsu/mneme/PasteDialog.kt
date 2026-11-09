package io.github.morangsu.mneme

import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.launch
import org.json.JSONObject

/**
 * 「粘贴整段教材」→ 自动填字段。
 *
 * 解析在 Rust 侧做（`parsePaste`），这里只负责展示：
 * 认出来的逐条列出让人核对，认不出来的原样留着让人手填 ——
 * 不猜、不静默丢弃，是这个产品所有交互的共同底线。
 */
@Composable
fun PasteFillDialog(
    kind: String,
    onDismiss: () -> Unit,
    onApply: (Map<String, String>) -> Unit,
) {
    var text by remember { mutableStateOf("") }
    var result by remember { mutableStateOf<JSONObject?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    fun parse() {
        if (busy || text.isBlank()) return
        busy = true
        error = null
        scope.launch {
            val r = Api.parsePaste(text, kind)
            busy = false
            val e = r.err()
            if (e != null) {
                error = e
                result = null
            } else {
                result = r
            }
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            color = MnemeColors.Bg,
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier
                .fillMaxWidth(0.94f)
                .fillMaxHeight(0.9f)
                .border(1.dp, MnemeColors.Divider, RoundedCornerShape(14.dp)),
        ) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(14.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Column {
                        Text("粘贴填充", color = MnemeColors.Text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Dim("每行一条「标签：值」，也认制表符与「标签 值」")
                    }
                    Text(
                        "关闭",
                        color = MnemeColors.Accent,
                        fontSize = 13.sp,
                        modifier = Modifier
                            .clickable { onDismiss() }
                            .padding(6.dp),
                    )
                }

                error?.let { ErrorBanner(it) }

                Column(
                    modifier = Modifier
                        .weight(1f)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    MnemeTextField(
                        value = text,
                        onValueChange = { text = it },
                        label = "例如：\n药名：麻黄\n功效：发汗解表、宣肺平喘\n性味：辛、微苦，温",
                        maxLines = 10,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        AccentButton(
                            text = "解析",
                            enabled = !busy && text.isNotBlank(),
                            onClick = { parse() },
                        )
                        if (result != null) {
                            GhostButton("清空", onClick = {
                                text = ""
                                result = null
                            })
                        }
                    }

                    val r = result
                    if (r != null) {
                        val hits = r.arrayOrEmpty("hits")
                        val unknown = r.arrayOrEmpty("unknown")
                        val conflicts = r.arrayOrEmpty("conflicts")
                        SectionTitle("识别 ${hits.length()} 项")
                        if (hits.length() == 0) {
                            Dim("一条都没认出来：检查标签是否与字段名对得上（如「功效」「性味归经」）")
                        }
                        for (i in 0 until hits.length()) {
                            val o = hits.optJSONObject(i) ?: continue
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.spacedBy(6.dp),
                            ) {
                                Text(
                                    "第 ${o.optInt("line")} 行",
                                    color = MnemeColors.TextDim,
                                    fontSize = 11.sp,
                                    modifier = Modifier.width(52.dp),
                                )
                                Text(
                                    o.optString("label"),
                                    color = MnemeColors.Accent,
                                    fontSize = 12.sp,
                                    modifier = Modifier.width(90.dp),
                                )
                                Text(
                                    o.optString("value"),
                                    color = MnemeColors.Text,
                                    fontSize = 12.sp,
                                )
                            }
                        }
                        for (i in 0 until conflicts.length()) {
                            Text(
                                "⚠ ${conflicts.optString(i)}",
                                color = MnemeColors.Warn,
                                fontSize = 11.sp,
                            )
                        }
                        if (unknown.length() > 0) {
                            SectionTitle("没认出来（会保留，请手动填）")
                            for (i in 0 until unknown.length()) {
                                Text(
                                    "· ${unknown.optString(i)}",
                                    color = MnemeColors.TextDim,
                                    fontSize = 12.sp,
                                )
                            }
                        }
                    }
                }

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    val r = result
                    val count = r?.arrayOrEmpty("hits")?.length() ?: 0
                    AccentButton(
                        text = if (count > 0) "填入 $count 项" else "填入",
                        enabled = count > 0 && !busy,
                        modifier = Modifier.weight(1f),
                        onClick = {
                            val out = mutableMapOf<String, String>()
                            val arr = r?.arrayOrEmpty("hits") ?: return@AccentButton
                            for (i in 0 until arr.length()) {
                                val o = arr.optJSONObject(i) ?: continue
                                out[o.optString("key")] = o.optString("value")
                            }
                            onApply(out)
                        },
                    )
                    GhostButton("取消", enabled = !busy, onClick = onDismiss)
                }
            }
        }
    }
}
