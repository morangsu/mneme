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

private data class QuizCard(val id: String, val prompt: String, val direction: String, val entityName: String)

private const val RETRY_DELAY_MS = 5 * 60 * 1000L

/**
 * 卡片复习（A2 闭环）：
 * - 每日队列：到期优先（越逾期越靠前）+ 新卡限量，当日稳定；
 * - 自动评分建议：判分对错 + 相似度 + 用时 → 1..4，用户可改；
 * - 会话内重试：答错的卡 5 分钟后回来，每卡本轮只重试一次；
 * - 错题本：连错两次进「薄弱」，连对自动移出（Rust 侧判定）；
 * - 掌握度：复习写事件时由 Rust 派生三档写回实体（手工覆盖 30 天内生效）。
 */
@Composable
fun QuizScreen() {
    var reload by remember { mutableIntStateOf(0) }
    var weakMode by remember { mutableStateOf(false) }
    var cards by remember { mutableStateOf<List<QuizCard>>(emptyList()) }
    var due by remember { mutableIntStateOf(0) }
    var newCount by remember { mutableIntStateOf(0) }
    var weakCount by remember { mutableIntStateOf(0) }
    var index by remember { mutableIntStateOf(0) }
    var answer by remember { mutableStateOf<JSONObject?>(null) }
    var verdict by remember { mutableStateOf<JSONObject?>(null) }
    var suggestion by remember { mutableStateOf<Pair<Int, String>?>(null) }
    var input by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var startedAt by remember { mutableLongStateOf(0L) }
    // 会话内重试：cardId -> 可重试的时间戳；retried 防止无限循环
    val retryAt = remember { mutableStateMapOf<String, Long>() }
    val retried = remember { mutableStateOf(setOf<String>()) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(reload, weakMode) {
        loading = true
        error = null
        val r = if (weakMode) Api.weakCards() else Api.quizQueue()
        loading = false
        if (r.err() != null) {
            error = r.err()
            return@LaunchedEffect
        }
        due = r.optInt("due", 0)
        newCount = r.optInt("new", 0)
        weakCount = r.optInt("weak", 0)
        val arr = r.arrayOrEmpty("cards", "items", "data")
        cards = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val id = o.optString("id", o.optString("card_id"))
            if (id.isBlank()) return@mapNotNull null
            QuizCard(id, o.optString("prompt"), o.optString("direction"), o.optString("entity_name"))
        }
        index = 0
        answer = null
        verdict = null
        suggestion = null
        input = ""
        startedAt = System.currentTimeMillis()
    }

    fun advance() {
        index++
        // 走到队尾时，把到时间的错题塞回来（每卡本轮只重试一次）
        while (index >= cards.size) {
            val now = System.currentTimeMillis()
            val ready = retryAt.entries.filter { it.value <= now }.map { it.key }
                .firstOrNull { id -> cards.none { it.id == id } } ?: break
            retryAt.remove(ready)
            val c = cards.firstOrNull { it.id == ready } ?: break
            cards = cards + c
        }
        answer = null
        verdict = null
        suggestion = null
        input = ""
        startedAt = System.currentTimeMillis()
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(12.dp)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Panel(title = if (weakMode) "薄弱模式" else "卡片复习") {
            if (weakMode) {
                Dim("错题集中练：连对两次自动移出错题本（判定在 Rust 侧）")
            } else {
                Dim("今日 ${due + newCount} 张（到期 $due / 新 $newCount）· 薄弱 $weakCount · 离线可用")
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                GhostButton("重新抽卡", onClick = { reload++ })
                if (!weakMode && weakCount > 0) {
                    GhostButton("薄弱 $weakCount 张", onClick = { weakMode = true })
                }
                if (weakMode) {
                    GhostButton("返回每日队列", onClick = { weakMode = false })
                }
            }
        }

        error?.let { ErrorBanner(it) }

        when {
            loading -> LoadingBox()

            cards.isEmpty() -> Dim(
                if (weakMode) "错题本是空的：连错两次的卡会出现在这里"
                else "今日没有到期的卡，也没有新卡。明天再来，或从公共库拉点内容。"
            )

            index >= cards.size -> Panel(title = "本轮完成") {
                Dim("已完成 ${cards.size} 张，评分与掌握度派生已写入 Rust 事件日志")
                if (retryAt.isNotEmpty()) {
                    Text(
                        "⚠ ${retryAt.size} 张错题将在 5 分钟后回来（或点「重新抽卡」立即检查）",
                        color = MnemeColors.Warn,
                        fontSize = 12.sp,
                    )
                }
                AccentButton("再来一轮", onClick = { reload++ })
            }

            else -> {
                val card = cards[index]
                LaunchedEffect(card.id, index) {
                    if (startedAt == 0L) startedAt = System.currentTimeMillis()
                }
                Panel(title = "第 ${index + 1}/${cards.size} 张 · ${card.direction}") {
                    if (card.entityName.isNotBlank()) {
                        Text(card.entityName, color = MnemeColors.TextDim, fontSize = 12.sp)
                    }
                    Text(
                        card.prompt,
                        color = MnemeColors.Text,
                        fontSize = 17.sp,
                        fontWeight = FontWeight.SemiBold,
                        lineHeight = 24.sp,
                    )

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
                            if (v.err() != null) {
                                error = v.err()
                            } else {
                                verdict = v
                                val s = Api.suggestGrade(
                                    v.optBoolean("correct", false),
                                    System.currentTimeMillis() - startedAt,
                                    v.optDouble("score", 0.0),
                                )
                                suggestion = s.optInt("grade", 3) to s.optString("reason", "")
                            }
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

                    SectionTitle("评分（写回 Rust 事件日志，掌握度自动派生）")
                    suggestion?.let { (g, reason) ->
                        Text(
                            "建议评分：${gradeLabel(g)} · $reason",
                            color = MnemeColors.Accent,
                            fontSize = 12.sp,
                        )
                    }
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        listOf(1 to "重来", 2 to "困难", 3 to "良好", 4 to "简单").forEach { (g, label) ->
                            GhostButton(
                                text = if (suggestion?.first == g) "$label ✓" else label,
                                modifier = Modifier.weight(1f),
                                onClick = {
                                    scope.launch {
                                        val r = Api.review(card.id, g.toUByte())
                                        if (r.err() != null) {
                                            error = r.err()
                                        } else {
                                            if (g == 1 && card.id !in retried.value) {
                                                retried.value = retried.value + card.id
                                                retryAt[card.id] =
                                                    System.currentTimeMillis() + RETRY_DELAY_MS
                                            }
                                            advance()
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

private fun gradeLabel(g: Int): String = when (g) {
    1 -> "重来"
    2 -> "困难"
    3 -> "良好"
    4 -> "简单"
    else -> g.toString()
}
