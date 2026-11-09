package io.github.morangsu.mneme

import androidx.compose.foundation.background
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
import org.json.JSONArray
import org.json.JSONObject

/**
 * 「粘贴整段教材」→ 自动填字段 / 直接建条目 / 批量建条目。
 *
 * 三种用法由调用方决定，解析规则完全一样（都在 Rust 侧）：
 * - `applyToForm`：填进新建表单（用户在表单里再核对一遍）；
 * - `afterCreate` 且只识别出 1 条：直接新建一条（OCR 草稿走这条）；
 * - `afterCreate` 且识别出多条：**空行分隔**批量新建（矩阵页「批量粘贴」）。
 *
 * 设计底线：认不出来的行逐条列出，绝不静默丢弃；批量新建逐条独立，
 * 一条重名不影响其它条落库。
 */
@Composable
fun PasteFillDialog(
    kind: String,
    initialText: String = "",
    applyToForm: ((Map<String, String>) -> Unit)? = null,
    afterCreate: (() -> Unit)? = null,
    onDismiss: () -> Unit,
) {
    var text by remember { mutableStateOf(initialText) }
    var primary by remember { mutableStateOf("name") }
    var blocks by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var skippedBlocks by remember { mutableIntStateOf(0) }
    var parsed by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    // 新建结果：每行 (文案, 是否是警告)
    var results by remember { mutableStateOf<List<Pair<String, Boolean>>>(emptyList()) }
    val scope = rememberCoroutineScope()

    fun parse() {
        if (busy || text.isBlank()) return
        busy = true
        error = null
        results = emptyList()
        scope.launch {
            val r = Api.parsePasteMany(text, kind)
            busy = false
            val e = r.err()
            if (e != null) {
                error = e
                return@launch
            }
            primary = r.optString("primary", "name").ifBlank { "name" }
            blocks = r.arrayOrEmpty("blocks").let { arr ->
                (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }
            }
            skippedBlocks = r.optInt("skipped_blocks", 0)
            parsed = true
        }
    }

    fun nameOf(b: JSONObject): String =
        b.optJSONObject("fields")?.optString(primary).orEmpty()

    fun create() {
        if (busy) return
        val items = JSONArray()
        blocks.forEach { b ->
            val f = b.optJSONObject("fields") ?: return@forEach
            items.put(JSONObject().put("fields", f))
        }
        if (items.length() == 0) return
        busy = true
        error = null
        scope.launch {
            val r = Api.createEntities(kind, items.toString())
            busy = false
            val e = r.err()
            if (e != null) {
                error = e
                return@launch
            }
            val lines = mutableListOf<Pair<String, Boolean>>()
            val created = r.arrayOrEmpty("created")
            for (i in 0 until created.length()) {
                val o = created.optJSONObject(i) ?: continue
                lines += "✓ 已新建《${o.optString("name")}》" to false
            }
            val skipped = r.arrayOrEmpty("skipped")
            for (i in 0 until skipped.length()) {
                val o = skipped.optJSONObject(i) ?: continue
                lines += "· 跳过：${o.optString("reason")}" to true
            }
            val failed = r.arrayOrEmpty("failed")
            for (i in 0 until failed.length()) {
                val o = failed.optJSONObject(i) ?: continue
                lines += "✗ 第 ${o.optInt("index") + 1} 条失败：${o.optString("error")}" to true
            }
            results = lines
            afterCreate?.invoke()
        }
    }

    val canFillForm = applyToForm != null && blocks.size == 1
    val canCreate = afterCreate != null && blocks.isNotEmpty()

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
                        Dim("每行一条「标签：值」；多条之间空一行")
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
                        onValueChange = {
                            text = it
                            parsed = false
                            results = emptyList()
                        },
                        label = "例如：\n药名：麻黄\n功效：发汗解表、宣肺平喘\n\n药名：桂枝\n功效：解肌",
                        maxLines = 10,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        AccentButton(
                            text = "解析",
                            enabled = !busy && text.isNotBlank(),
                            onClick = { parse() },
                        )
                        if (parsed) {
                            GhostButton("清空", onClick = {
                                text = ""
                                blocks = emptyList()
                                parsed = false
                                results = emptyList()
                            })
                        }
                    }

                    if (parsed) {
                        if (blocks.isEmpty()) {
                            Dim("一条都没认出来：检查标签是否与字段名对得上（如「功效」「性味归经」）")
                        } else {
                            SectionTitle(
                                if (blocks.size == 1) "识别 ${blocks[0].optInt("matched")} 项"
                                else "识别到 ${blocks.size} 条"
                            )
                        }
                        if (skippedBlocks > 0) {
                            Dim("另有 $skippedBlocks 个空段被跳过")
                        }
                        blocks.forEachIndexed { i, b ->
                            val hits = b.arrayOrEmpty("hits")
                            val unknown = b.arrayOrEmpty("unknown")
                            val conflicts = b.arrayOrEmpty("conflicts")
                            val name = nameOf(b)
                            Column(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .background(MnemeColors.Panel, RoundedCornerShape(8.dp))
                                    .padding(8.dp),
                                verticalArrangement = Arrangement.spacedBy(3.dp),
                            ) {
                                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    Badge("第 ${i + 1} 条", MnemeColors.TextDim)
                                    Text(
                                        name.ifBlank { "（没有药名）" },
                                        color = if (name.isBlank()) MnemeColors.Warn else MnemeColors.Text,
                                        fontSize = 13.sp,
                                        fontWeight = FontWeight.Medium,
                                    )
                                    Dim("${hits.length()} 项")
                                }
                                val labels = (0 until hits.length())
                                    .mapNotNull { hits.optJSONObject(it)?.optString("label") }
                                    .filter { it.isNotBlank() }
                                if (labels.isNotEmpty()) {
                                    Text(labels.joinToString("、"), color = MnemeColors.Accent, fontSize = 11.sp)
                                }
                                for (k in 0 until conflicts.length()) {
                                    Text("⚠ ${conflicts.optString(k)}", color = MnemeColors.Warn, fontSize = 11.sp)
                                }
                                for (k in 0 until unknown.length()) {
                                    Text(
                                        "· 未识别：${unknown.optString(k)}",
                                        color = MnemeColors.TextDim,
                                        fontSize = 11.sp,
                                    )
                                }
                            }
                        }

                        if (results.isNotEmpty()) {
                            SectionTitle("结果")
                            results.forEach { line ->
                                Text(
                                    line.first,
                                    color = if (line.second) MnemeColors.Warn else MnemeColors.Ok,
                                    fontSize = 12.sp,
                                )
                            }
                        }
                    }
                }

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (canFillForm) {
                        val n = blocks[0].optInt("matched")
                        AccentButton(
                            text = "填入 $n 项",
                            enabled = n > 0 && !busy,
                            modifier = Modifier.weight(1f),
                            onClick = {
                                val out = mutableMapOf<String, String>()
                                val hits = blocks[0].arrayOrEmpty("hits")
                                for (i in 0 until hits.length()) {
                                    val o = hits.optJSONObject(i) ?: continue
                                    out[o.optString("key")] = o.optString("value")
                                }
                                applyToForm?.invoke(out)
                            },
                        )
                    } else if (canCreate) {
                        AccentButton(
                            text = if (blocks.size == 1) "新建这条" else "全部新建 ${blocks.size} 条",
                            enabled = !busy && results.isEmpty(),
                            modifier = Modifier.weight(1f),
                            onClick = { create() },
                        )
                    } else if (blocks.size > 1 && applyToForm != null) {
                        Dim("多条请在矩阵页用「批量粘贴」建，表单只能填一条")
                    }
                    GhostButton(
                        text = if (results.isNotEmpty()) "完成" else "取消",
                        enabled = !busy,
                        onClick = onDismiss,
                    )
                }
            }
        }
    }
}
