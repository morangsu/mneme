package io.github.morangsu.mneme

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import org.json.JSONObject

private data class QuizCard(val id: String, val prompt: String, val direction: String)

/** 卡片复习：出题 / 答案 / 判分 / 四档评分都走 Rust FFI，本地无需网络，离线可用。 */
@Composable
fun QuizScreen() {
    var reload by remember { mutableIntStateOf(0) }
    var cards by remember { mutableStateOf<List<QuizCard>>(emptyList()) }
    var due by remember { mutableIntStateOf(0) }
    var index by remember { mutableIntStateOf(0) }
    var answer by remember { mutableStateOf<JSONObject?>(null) }
    var verdict by remember { mutableStateOf<JSONObject?>(null) }
    var input by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(reload) {
        loading = true
        error = null
        val r = Api.quiz(5u)
        loading = false
        if (r.err() != null) {
            error = r.err()
            return@LaunchedEffect
        }
        due = r.optInt("due", 0)
        val arr = r.arrayOrEmpty("cards", "data")
        cards = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            QuizCard(o.optString("id"), o.optString("prompt"), o.optString("direction"))
        }
        index = 0
        answer = null
        verdict = null
        input = ""
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(12.dp)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Panel(title = "卡片复习") {
            Dim("到期 $due 张 · 本轮 ${cards.size} 张 · 离线可用（出题与判分都在 Rust 侧）")
            GhostButton("重新抽卡", onClick = { reload++ })
        }

        error?.let { ErrorBanner(it) }

        when {
            loading -> LoadingBox()

            cards.isEmpty() -> Dim("暂无卡片，先导入数据或加载领域包")

            index >= cards.size -> Panel(title = "本轮完成") {
                Dim("已完成 ${cards.size} 张卡片的复习，评分已写入 Rust 事件日志")
                AccentButton("再来一轮", onClick = { reload++ })
            }

            else -> {
                val card = cards[index]
                Panel(title = "第 ${index + 1}/${cards.size} 张 · ${card.direction}") {
                    Text(
                        card.prompt,
                        color = MnemeColors.Text,
                        fontSize = 17.sp,
                        fontWeight = FontWeight.SemiBold,
                        lineHeight = 24.sp,
                    )
                    Dim("卡片 id：${card.id}")

                    GhostButton("显示答案", onClick = {
                        scope.launch {
                            val a = Api.answer(card.id)
                            if (a.err() != null) error = a.err() else answer = a
                        }
                    })

                    val a = answer
                    if (a != null) {
                        Panel {
                            SectionTitle("参考答案")
                            Text(
                                a.optString("answer", "—"),
                                color = MnemeColors.Accent,
                                fontSize = 16.sp,
                                fontWeight = FontWeight.SemiBold,
                            )
                            val accept = a.readable("accept")
                            if (accept.isNotBlank()) Dim("可接受表述：$accept")
                            val boundary = a.optString("boundary", "")
                            if (boundary.isNotBlank()) {
                                Text("⚠ 边界：$boundary", color = MnemeColors.Warn, fontSize = 12.sp)
                            }
                        }
                    }

                    MnemeTextField(
                        value = input,
                        onValueChange = { input = it },
                        label = "自由输入答案（同义归一 + 容错判分）",
                        singleLine = true,
                    )
                    AccentButton("判分", enabled = input.isNotBlank(), onClick = {
                        scope.launch {
                            val v = Api.judge(card.id, input)
                            if (v.err() != null) error = v.err() else verdict = v
                        }
                    })

                    val v = verdict
                    if (v != null) {
                        val correct = v.optBoolean("correct", false)
                        Text(
                            (if (correct) "✓ 判为正确" else "✗ 判为不正确") +
                                " · 相似度 ${v.optDouble("score", 0.0)}" +
                                " · 命中 ${v.readable("matched").ifBlank { "无" }}",
                            color = if (correct) MnemeColors.Ok else MnemeColors.Danger,
                            fontSize = 13.sp,
                        )
                    }

                    SectionTitle("评分（写回 Rust 事件日志）")
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        listOf(1 to "重来", 2 to "一般", 3 to "良好", 4 to "简单").forEach { (g, label) ->
                            GhostButton(
                                text = label,
                                modifier = Modifier.weight(1f),
                                onClick = {
                                    scope.launch {
                                        val r = Api.review(card.id, g.toUByte())
                                        if (r.err() != null) {
                                            error = r.err()
                                        } else {
                                            index++
                                            answer = null
                                            verdict = null
                                            input = ""
                                        }
                                    }
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}
