package io.github.morangsu.mneme

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

private const val KEEP_MINE = "KeepMine"
private const val TAKE_UPSTREAM = "TakeUpstream"
private const val KEEP_BOTH = "KeepBoth"

private data class Conflict(
    val id: String,
    val entityName: String,
    val field: String,
    val label: String,
    val base: String,
    val upstream: String,
    val local: String,
    val immutable: Boolean,
    val personalized: Boolean,
    val suggestion: String,
    val kind: String,
    val page: Int?,
)

private data class Choice(val decision: String? = null, val custom: String = "")

/** 兼容 Rust serde 变体名、CLI FromStr 别名、中文文案等各种 suggestion 取值。 */
private fun normalizeDecision(raw: String): String = when (raw.lowercase()) {
    "keep_mine", "keepmine", "mine", "k", "保留我的" -> KEEP_MINE
    "take_upstream", "takeupstream", "upstream", "u", "采用上游" -> TAKE_UPSTREAM
    "keep_both", "keepboth", "both", "b", "两者都留" -> KEEP_BOTH
    else -> raw.ifBlank { KEEP_MINE }
}

private fun decisionLabel(d: String): String = when (d) {
    KEEP_MINE -> "保留我的"
    TAKE_UPSTREAM -> "采用上游"
    KEEP_BOTH -> "两者都留"
    else -> d
}

private fun kindLabel(k: String): String = when (k) {
    "BothChanged" -> "双方都改"
    "ImmutableClash" -> "原样字段冲突"
    "UpstreamRemovedLocallyEdited" -> "上游移除(本地改过)"
    else -> k.ifBlank { "字段冲突" }
}

/**
 * 冲突裁决（产品硬规则）：
 * 公共基础库拉取产生的冲突必须由用户**逐项决定**，三值并列展示、三选一或自定义值；
 * 「全部按建议」只是批量填充决定，仍需用户主动提交；
 * **未全部决定前提交按钮禁用，Rust 侧也不会写入任何数据**（对应 apply_decisions 的 Unresolved）。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConflictScreen(epoch: Int) {
    var plan by remember { mutableStateOf<JSONObject?>(null) }
    var conflicts by remember { mutableStateOf<List<Conflict>>(emptyList()) }
    var choices by remember { mutableStateOf<Map<String, Choice>>(emptyMap()) }
    var report by remember { mutableStateOf<JSONObject?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(epoch) {
        busy = true
        error = null
        report = null
        val p = Api.upstreamPull()
        busy = false
        if (p.err() != null) {
            error = p.err()
            return@LaunchedEffect
        }
        plan = p
        val arr = p.arrayOrEmpty("conflicts")
        conflicts = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            Conflict(
                id = o.optString("id"),
                entityName = o.optString("entity_name", o.optString("entity_id", "?")),
                field = o.optString("field"),
                label = o.optString("label", o.optString("field")),
                base = o.optString("base"),
                upstream = o.optString("upstream"),
                local = o.optString("local"),
                immutable = o.optBoolean("immutable"),
                personalized = o.optBoolean("personalized"),
                suggestion = normalizeDecision(o.optString("suggestion", KEEP_MINE)),
                kind = o.optString("kind", ""),
                page = if (o.has("page") && !o.isNull("page")) o.optInt("page") else null,
            )
        }
        choices = emptyMap()
    }

    val decidedCount = conflicts.count { choices[it.id]?.decision != null }
    val allDecided = conflicts.isNotEmpty() && decidedCount == conflicts.size

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Panel(title = "公共库拉取 · 冲突裁决") {
            val p = plan
            if (p != null) {
                val summary = p.optString("summary", "")
                Text(
                    summary.ifBlank {
                        "上游 r${p.optInt("from_rev")} → r${p.optInt("to_rev")} · " +
                            "自动合并 ${p.arrayOrEmpty("auto").length()} 项 · " +
                            "新增 ${p.arrayOrEmpty("additions").length()} · " +
                            "移除 ${p.arrayOrEmpty("removals").length()} · " +
                            "待裁决 ${conflicts.size} 项"
                    },
                    color = MnemeColors.Text,
                    fontSize = 13.sp,
                )
            } else {
                Dim("正在生成拉取计划…")
            }
            Dim("已决定 $decidedCount / ${conflicts.size} 项。公共库拉取的冲突必须由用户逐项决定，未全部决定前不写任何数据。")

            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                GhostButton("全部按建议", enabled = conflicts.isNotEmpty(), onClick = {
                    choices = conflicts.associate { c ->
                        c.id to Choice(
                            decision = c.suggestion,
                            custom = choices[c.id]?.custom ?: "",
                        )
                    }
                })
                AccentButton("提交裁决", enabled = allDecided && !busy, onClick = {
                    val current = plan
                    if (current != null) {
                        scope.launch {
                            busy = true
                            error = null
                            val json = buildDecisionsJson(conflicts, choices)
                            val r = Api.upstreamResolve(current.optString("id"), json)
                            busy = false
                            if (r.err() != null) error = r.err() else report = r
                        }
                    }
                })
            }
            if (conflicts.isNotEmpty() && !allDecided) {
                Text(
                    "⚠ 仍有 ${conflicts.size - decidedCount} 项未决定，提交按钮已禁用",
                    color = MnemeColors.Warn,
                    fontSize = 12.sp,
                )
            }
        }

        if (busy) LoadingBox()
        error?.let { ErrorBanner(it) }
        report?.let { Panel(title = "裁决回执") { Receipt(it) } }

        when {
            conflicts.isEmpty() && !busy -> Panel {
                Dim("没有待裁决冲突，公共库内容可自动合并。")
            }

            else -> LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                conflicts.forEach { c ->
                    item(key = c.id) {
                        ConflictCard(
                            conflict = c,
                            choice = choices[c.id] ?: Choice(),
                            onChoice = { decision ->
                                choices = choices + (c.id to (choices[c.id] ?: Choice()).copy(decision = decision))
                            },
                            onCustom = { value ->
                                choices = choices + (c.id to (choices[c.id] ?: Choice()).copy(custom = value))
                            },
                        )
                    }
                }
            }
        }
    }
}

/** 构造 Rust `Resolution[]`：[{conflict_id, decision, custom_value}, ...]，decision 用 serde 变体名。 */
private fun buildDecisionsJson(conflicts: List<Conflict>, choices: Map<String, Choice>): String {
    val arr = JSONArray()
    conflicts.forEach { c ->
        val choice = choices[c.id] ?: return@forEach
        val decision = choice.decision ?: return@forEach
        val o = JSONObject()
            .put("conflict_id", c.id)
            .put("decision", decision)
        if (choice.custom.isNotBlank()) {
            o.put("custom_value", choice.custom)
        } else {
            o.put("custom_value", JSONObject.NULL)
        }
        arr.put(o)
    }
    return arr.toString()
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ConflictCard(
    conflict: Conflict,
    choice: Choice,
    onChoice: (String) -> Unit,
    onCustom: (String) -> Unit,
) {
    val decision = choice.decision
    Panel {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(
                "${conflict.entityName} · ${conflict.label}",
                color = MnemeColors.Text,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f),
            )
            if (conflict.personalized) Badge("个性化", MnemeColors.Ok)
            if (conflict.immutable) Badge("不可改·原样", MnemeColors.Danger)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Badge(kindLabel(conflict.kind), MnemeColors.TextDim)
            Badge("建议：${decisionLabel(conflict.suggestion)}", MnemeColors.Warn)
            conflict.page?.let { Dim("教材 p.$it") }
        }
        Dim("字段 ${conflict.field} · 冲突 id ${conflict.id}")

        // 三列并排：base / 上游新值 / 我的值，选中态高亮对应列。
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            TriColumn("原始 base", conflict.base, MnemeColors.TextDim, highlighted = false)
            TriColumn(
                "上游新值",
                conflict.upstream,
                MnemeColors.Accent,
                highlighted = decision == TAKE_UPSTREAM || decision == KEEP_BOTH,
            )
            TriColumn(
                "我的值",
                conflict.local,
                MnemeColors.Ok,
                highlighted = decision == KEEP_MINE || decision == KEEP_BOTH,
            )
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            listOf(
                KEEP_MINE to "保留我的",
                TAKE_UPSTREAM to "采用上游",
                KEEP_BOTH to "两者都留",
            ).forEach { (value, label) ->
                FilterChip(
                    selected = decision == value,
                    onClick = { onChoice(value) },
                    label = { Text(label, fontSize = 11.sp) },
                    modifier = Modifier.weight(1f),
                    colors = FilterChipDefaults.filterChipColors(
                        selectedContainerColor = MnemeColors.Accent.copy(alpha = 0.22f),
                        selectedLabelColor = MnemeColors.Accent,
                    ),
                )
            }
        }

        MnemeTextField(
            value = choice.custom,
            onValueChange = onCustom,
            label = "自定义值（可选，优先于以上三选一）",
            maxLines = 2,
        )

        if (decision == null) {
            Text("未决定", color = MnemeColors.Warn, fontSize = 11.sp)
        } else {
            Text("已选：${decisionLabel(decision)}", color = MnemeColors.Accent, fontSize = 11.sp)
        }
    }
}

@Composable
private fun RowScope.TriColumn(
    title: String,
    value: String,
    color: Color,
    highlighted: Boolean,
) {
    Column(
        modifier = Modifier
            .weight(1f)
            .heightIn(min = 76.dp)
            .background(
                if (highlighted) color.copy(alpha = 0.12f) else MnemeColors.PanelAlt,
                RoundedCornerShape(6.dp),
            )
            .border(
                if (highlighted) 1.dp else 0.5.dp,
                if (highlighted) color else MnemeColors.Divider,
                RoundedCornerShape(6.dp),
            )
            .padding(6.dp),
    ) {
        Text(title, color = color, fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(4.dp))
        Text(
            value.ifBlank { "—" },
            color = MnemeColors.Text,
            fontSize = 11.sp,
            maxLines = 5,
            overflow = TextOverflow.Ellipsis,
        )
    }
}
