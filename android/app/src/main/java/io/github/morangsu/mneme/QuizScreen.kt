package io.github.morangsu.mneme

import androidx.compose.foundation.background
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
import org.json.JSONArray
import org.json.JSONObject

/**
 * 一张待复习的卡，连同它的记忆状态。
 * 记忆状态直接摊给用户看 —— 「还有 92% 概率记得」比「下次复习 3 天后」更好理解。
 */
private data class QuizCard(
    val id: String,
    val prompt: String,
    val direction: String,
    val entityName: String,
    /** 答案就是药名时为 true —— 这种卡绝不能提前显示药名，否则等于直接给答案 */
    val askName: Boolean,
    val stability: Double,
    val retrievability: Double,
    val lapses: Int,
    val leech: Boolean,
)

private const val RETRY_DELAY_MS = 5 * 60 * 1000L
private const val DAY_MS = 86_400_000.0

/**
 * 卡片复习（A2 闭环 + FSRS-5）：
 * - 每日队列：到期优先（越逾期越靠前）+ 新卡限量，数量可在「复习设置」里改；
 * - 本轮张数：这一次会话拿多少张，与每日配额分开，随时可调；
 * - 自动评分建议：判分对错 + 相似度 + 用时 → 1..4，用户可改；
 * - 会话内重试：答错的卡 5 分钟后回来，每卡本轮只重试一次；
 * - 错题本 / leech：连错两次进「薄弱」，累计遗忘到阈值判 leech（建议改写这张卡）；
 * - 掌握度：复习写事件时由 Rust 派生三档写回实体（手工覆盖 30 天内生效）。
 */
@Composable
fun QuizScreen() {
    var reload by remember { mutableIntStateOf(0) }
    var weakMode by remember { mutableStateOf(false) }
    var settingsOpen by remember { mutableStateOf(false) }

    // pool 是本次抽到的全部卡（重试时从这里取），queue 是还要走的顺序
    var pool by remember { mutableStateOf<List<QuizCard>>(emptyList()) }
    var queue by remember { mutableStateOf<List<QuizCard>>(emptyList()) }

    var due by remember { mutableIntStateOf(0) }
    var newCount by remember { mutableIntStateOf(0) }
    var weakCount by remember { mutableIntStateOf(0) }
    var leechCount by remember { mutableIntStateOf(0) }
    var totalInQueue by remember { mutableIntStateOf(0) }
    var sessionLimit by remember { mutableIntStateOf(0) }
    var retention by remember { mutableDoubleStateOf(0.9) }

    var index by remember { mutableIntStateOf(0) }
    var answer by remember { mutableStateOf<JSONObject?>(null) }
    var verdict by remember { mutableStateOf<JSONObject?>(null) }
    var suggestion by remember { mutableStateOf<Pair<Int, String>?>(null) }
    var input by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var startedAt by remember { mutableLongStateOf(0L) }
    // 最近一次复习的回执（掌握度派生结果）。放在顶层，跨卡保留几秒。
    var receipt by remember { mutableStateOf<String?>(null) }

    // 会话内重试：cardId -> 可重试时间；retried 保证每卡本轮最多补一次
    val retryAt = remember { mutableStateMapOf<String, Long>() }
    val retried = remember { mutableStateOf(setOf<String>()) }
    val scope = rememberCoroutineScope()

    suspend fun load() {
        loading = true
        error = null
        val r = if (weakMode) Api.weakCards(limit = 100u) else Api.quizPlan()
        loading = false
        if (r.err() != null) {
            error = r.err()
            return
        }
        due = r.optInt("due", 0)
        newCount = r.optInt("new", 0)
        weakCount = r.optInt("weak", 0)
        leechCount = r.optInt("leech", 0)
        totalInQueue = r.optInt("total", 0)
        sessionLimit = r.optInt("session_limit", 0)
        retention = r.optDouble("retention", 0.9)

        val arr = r.arrayOrEmpty("cards", "items", "data")
        val list = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val id = o.optString("id", o.optString("card_id"))
            if (id.isBlank()) return@mapNotNull null
            QuizCard(
                id = id,
                prompt = o.optString("prompt"),
                direction = o.optString("direction"),
                entityName = o.optString("entity_name"),
                askName = o.optBoolean("ask_name", false),
                stability = o.optDouble("stability", 0.0),
                retrievability = o.optDouble("retrievability", -1.0),
                lapses = o.optInt("lapses", 0),
                leech = o.optBoolean("leech", false),
            )
        }
        pool = list
        queue = list
        receipt = null
        index = 0
        answer = null
        verdict = null
        suggestion = null
        input = ""
        retryAt.clear()
        retried.value = emptySet()
        startedAt = System.currentTimeMillis()
    }

    LaunchedEffect(reload, weakMode) { load() }

    // 走到队尾时，把到时间的错题补进队列；一次只补一张，下一次 advance 继续判断
    fun advance() {
        while (index >= queue.size) {
            val now = System.currentTimeMillis()
            val ready = retryAt.entries.filter { it.value <= now }.minByOrNull { it.value }?.key
                ?: break
            retryAt.remove(ready)
            val c = pool.firstOrNull { it.id == ready } ?: continue
            queue = queue + c
            break
        }
        index++
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
                Dim(
                    "今日 ${due + newCount} 张（到期 $due / 新 $newCount）· 薄弱 $weakCount" +
                        if (leechCount > 0) " · 顽固 $leechCount" else ""
                )
                Dim(
                    if (sessionLimit > 0) {
                        "本轮取 ${minOf(sessionLimit, totalInQueue)} 张 · 期望记忆率 ${(retention * 100).toInt()}%"
                    } else {
                        "本轮不限张数 · 期望记忆率 ${(retention * 100).toInt()}%"
                    }
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                GhostButton("重新抽卡", onClick = { reload++ })
                if (!weakMode) {
                    GhostButton("复习设置", onClick = { settingsOpen = true })
                }
                if (!weakMode && weakCount > 0) {
                    GhostButton("薄弱 $weakCount 张", onClick = { weakMode = true })
                }
                if (weakMode) {
                    GhostButton("返回每日队列", onClick = { weakMode = false })
                }
            }
        }

        error?.let { ErrorBanner(it) }
        receipt?.let {
            Text(
                "✓ $it",
                color = MnemeColors.Ok,
                fontSize = 12.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(MnemeColors.Ok.copy(alpha = 0.10f), RoundedCornerShape(8.dp))
                    .padding(8.dp),
            )
        }

        when {
            loading -> LoadingBox()

            queue.isEmpty() -> Dim(
                if (weakMode) "错题本是空的：连错两次的卡会出现在这里"
                else "今日没有到期的卡，也没有新卡。明天再来，或从公共库拉点内容。"
            )

            index >= queue.size -> Panel(title = "本轮完成") {
                Dim("已完成 ${queue.size} 张，评分与掌握度派生已写入 Rust 事件日志")
                if (retryAt.isNotEmpty()) {
                    Text(
                        "⚠ ${retryAt.size} 张错题还没到重试时间（5 分钟），点「重新抽卡」会立刻重新检查",
                        color = MnemeColors.Warn,
                        fontSize = 12.sp,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AccentButton("再来一轮", onClick = { reload++ })
                    GhostButton("复习设置", onClick = { settingsOpen = true })
                }
            }

            else -> {
                val card = queue[index]
                Panel(title = "第 ${index + 1}/${queue.size} 张 · ${card.direction}") {
                    // 只在「不泄题」时显示药名：答药名的卡（*_to_name）一显示就等于送答案；
                    // 题干里已经出现药名时也不必再重复一遍。
                    val mayShowName = !card.askName && !card.prompt.contains(card.entityName)
                    if (card.entityName.isNotBlank() && mayShowName) {
                        Text(card.entityName, color = MnemeColors.TextDim, fontSize = 12.sp)
                    }
                    Text(
                        card.prompt,
                        color = MnemeColors.Text,
                        fontSize = 17.sp,
                        fontWeight = FontWeight.SemiBold,
                        lineHeight = 24.sp,
                    )

                    // 记忆状态：S 是「记忆稳定度（天）」，R 是此刻还记得的概率
                    if (card.stability <= 0) {
                        // 从未复习过：明确标出来，否则「没有徽章」会被误读成加载失败
                        Badge("新卡")
                    } else {
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Badge("S ${fmtDays(card.stability)}")
                            if (card.retrievability >= 0) {
                                Badge(
                                    "记得 ${(card.retrievability * 100).toInt()}%",
                                    if (card.retrievability < 0.85) MnemeColors.Warn
                                    else MnemeColors.Ok,
                                )
                            }
                            if (card.lapses > 0) Badge("忘过 ${card.lapses} 次", MnemeColors.TextDim)
                            if (card.leech) Badge("顽固卡", MnemeColors.Danger)
                        }
                        if (card.leech) {
                            Text(
                                "这张卡已累计遗忘到阈值：继续硬背收益很低，建议「复制为新」换个记忆线索。",
                                color = MnemeColors.Danger,
                                fontSize = 11.sp,
                            )
                        }
                    }

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
                    // 四档对应 FSRS 的 1/2/3/4：重来会缩短稳定度并回到重学步骤，
                    // 简单给 easy bonus。标签用中文，括注实际效果避免误解。
                    Dim("重来=忘了 · 困难=勉强想起 · 良好=正常想起 · 简单=脱口而出")
                    // 四个挤一行会把「重来」压成两行（实测 1080px 屏），改两行各两个
                    listOf(
                        listOf(1 to "重来", 2 to "困难"),
                        listOf(3 to "良好", 4 to "简单"),
                    ).forEach { pair ->
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        pair.forEach { (g, label) ->
                            GhostButton(
                                text = if (suggestion?.first == g) "$label ✓" else label,
                                modifier = Modifier.weight(1f),
                                onClick = {
                                    scope.launch {
                                        val r = Api.review(card.id, g.toUByte())
                                        if (r.err() != null) {
                                            error = r.err()
                                        } else {
                                            val who = card.entityName.ifBlank { card.prompt.take(12) }
                                            val m = r.optJSONObject("mastery")
                                            // 覆盖率不足时解释一句，否则「明明答对了还是熟悉」说不通
                                            val lag = if (m != null) {
                                                val rest = m.optInt("total", 0) - m.optInt("reviewed", 0)
                                                if (rest > 0) "（还有 $rest 张卡没学过）" else ""
                                            } else ""
                                            receipt = if (m != null && m.optBoolean("changed", false)) {
                                                "$who 掌握度 ${m.optString("prev").ifBlank { "—" }} → ${m.optString("label")}$lag"
                                            } else if (m != null && m.optBoolean("overridden", false)) {
                                                "$who 掌握度已手工设定（30 天内不被覆盖）"
                                            } else {
                                                "已记录：$who · ${gradeLabel(g)}"
                                            }
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

    if (settingsOpen) {
        ReviewSettingsDialog(
            onDismiss = { settingsOpen = false },
            onSaved = {
                settingsOpen = false
                reload++
            },
        )
    }
}

/**
 * 复习设置：每日新卡/到期上限、本轮张数、期望记忆率、学习步骤、leech 阈值。
 *
 * 越界值由 Rust 侧 `normalized()` 夹回（这里也做一次，让用户立刻看到被纠正成什么）。
 */
@Composable
private fun ReviewSettingsDialog(onDismiss: () -> Unit, onSaved: () -> Unit) {
    var newPerDay by remember { mutableStateOf("10") }
    var reviewsPerDay by remember { mutableStateOf("100") }
    var session by remember { mutableStateOf("20") }
    var retention by remember { mutableStateOf("0.90") }
    var steps by remember { mutableStateOf("1,10") }
    var leech by remember { mutableStateOf("8") }
    var maxDays by remember { mutableStateOf("3650") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var loaded by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        val c = Api.reviewConfig()
        if (c.err() == null) {
            newPerDay = c.optInt("new_per_day", 10).toString()
            reviewsPerDay = c.optInt("reviews_per_day", 100).toString()
            session = c.optInt("session_limit", 20).toString()
            retention = c.optDouble("desired_retention", 0.9).toString()
            leech = c.optInt("leech_threshold", 8).toString()
            maxDays = c.optInt("max_interval_days", 3650).toString()
            val arr = c.arrayOrEmpty("learning_steps_minutes")
            steps = if (arr.length() == 0) "1,10" else (0 until arr.length())
                .map { trimNum(arr.optDouble(it)) }
                .joinToString(",")
        }
        loaded = true
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Surface(
            color = MnemeColors.Panel,
            shape = RoundedCornerShape(12.dp),
            modifier = Modifier
                .fillMaxWidth(0.94f)
                .fillMaxHeight(0.86f),
        ) {
            Column(
                modifier = Modifier
                    .padding(16.dp)
                    .verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Text("复习设置", color = MnemeColors.Text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Dim("算法：FSRS-5（DSR 模型）。参数保存在本地 review.json，不上传。")
                if (!loaded) LoadingBox()
                error?.let { ErrorBanner(it) }

                MnemeTextField(newPerDay, { newPerDay = it }, "每日新卡上限（张）", singleLine = true)
                MnemeTextField(reviewsPerDay, { reviewsPerDay = it }, "每日到期上限（张）", singleLine = true)
                MnemeTextField(session, { session = it }, "本轮张数（0 = 不限）", singleLine = true)
                MnemeTextField(
                    retention,
                    { retention = it },
                    "期望记忆率（0.70–0.97，越高复习越勤）",
                    singleLine = true,
                )
                MnemeTextField(steps, { steps = it }, "学习步骤（分钟，逗号分隔，如 1,10）", singleLine = true)
                MnemeTextField(leech, { leech = it }, "顽固卡阈值（累计遗忘次数）", singleLine = true)
                MnemeTextField(maxDays, { maxDays = it }, "最大间隔（天）", singleLine = true)

                Dim("提示：期望记忆率 0.9 → 复习时约 90% 还记得；降到 0.85 间隔变长、复习变少。")

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    AccentButton(
                        text = "保存",
                        enabled = !busy && loaded,
                        modifier = Modifier.weight(1f),
                        onClick = {
                            busy = true
                            error = null
                            scope.launch {
                                val stepArr = JSONArray()
                                steps.split(',', '，', ' ')
                                    .mapNotNull { it.trim().toDoubleOrNull() }
                                    .forEach { stepArr.put(it) }
                                val body = JSONObject()
                                    .put("new_per_day", newPerDay.trim().toIntOrNull() ?: 10)
                                    .put("reviews_per_day", reviewsPerDay.trim().toIntOrNull() ?: 100)
                                    .put("session_limit", session.trim().toIntOrNull() ?: 20)
                                    .put(
                                        "desired_retention",
                                        retention.trim().toDoubleOrNull() ?: 0.9,
                                    )
                                    .put("leech_threshold", leech.trim().toIntOrNull() ?: 8)
                                    .put("max_interval_days", maxDays.trim().toIntOrNull() ?: 3650)
                                    .put("easy_interval_days", 4)
                                    .put("learning_steps_minutes", stepArr)
                                val r = Api.setReviewConfig(body.toString())
                                busy = false
                                val e = r.err()
                                if (e != null) {
                                    error = e
                                } else {
                                    // 用服务端夹回后的值回填，用户能立刻看到「被纠正成什么」
                                    val cfg = r.optJSONObject("config")
                                    if (cfg != null) {
                                        newPerDay = cfg.optInt("new_per_day", 10).toString()
                                        reviewsPerDay = cfg.optInt("reviews_per_day", 100).toString()
                                        session = cfg.optInt("session_limit", 20).toString()
                                        retention = cfg.optDouble("desired_retention", 0.9).toString()
                                        leech = cfg.optInt("leech_threshold", 8).toString()
                                    }
                                    onSaved()
                                }
                            }
                        },
                    )
                    GhostButton("取消", enabled = !busy, onClick = onDismiss)
                }
            }
        }
    }
}

private fun trimNum(v: Double): String =
    if (v == v.toLong().toDouble()) v.toLong().toString() else v.toString()

private fun fmtDays(stability: Double): String = when {
    stability < 1.0 -> "${(stability * 24).toInt()} 小时"
    stability < 30.0 -> "${(stability * 10).toInt() / 10.0} 天"
    else -> "${(stability / 30).toInt()} 个月"
}

private fun gradeLabel(g: Int): String = when (g) {
    1 -> "重来"
    2 -> "困难"
    3 -> "良好"
    4 -> "简单"
    else -> g.toString()
}
