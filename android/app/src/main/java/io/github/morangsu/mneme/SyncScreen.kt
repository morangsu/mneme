package io.github.morangsu.mneme

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import org.json.JSONObject

/**
 * 同步页：目标列表 / 立即同步与预演 / 回执 / 公共基础库拉取 / 导入导出 / OCR 入口。
 * 网络同步、三方合并、导入导出全部在 Rust 侧，APP 只负责发起与展示结果。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SyncScreen(onGoConflict: () -> Unit) {
    var targets by remember { mutableStateOf(listOf("github-private", "github-public", "local")) }
    var target by remember { mutableStateOf("github-private") }
    var status by remember { mutableStateOf<JSONObject?>(null) }
    var receipt by remember { mutableStateOf<JSONObject?>(null) }
    var pullPlan by remember { mutableStateOf<JSONObject?>(null) }
    var importResult by remember { mutableStateOf<JSONObject?>(null) }
    var exportResult by remember { mutableStateOf<JSONObject?>(null) }
    var importText by remember { mutableStateOf("") }
    var importPreview by remember { mutableStateOf<JSONObject?>(null) }
    // OCR / 粘贴的文字如果要「当一条药」，得知道当前领域的主实体类型
    var domainKind by remember { mutableStateOf("herb") }
    var draftOpen by remember { mutableStateOf(false) }
    var exportFormat by remember { mutableStateOf("bundle") }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        val s = Api.upstreamStatus()
        if (s.err() == null) {
            status = s
            val arr = s.arrayOrEmpty("targets")
            if (arr.length() > 0) {
                targets = (0 until arr.length()).map { arr.optString(it) }
                if (target !in targets) target = targets.first()
            }
        }
    }

    LaunchedEffect(Unit) {
        val st = Api.state()
        if (st.err() == null) {
            domainKind = st.optString("kind").ifBlank { "herb" }
        }
    }

    fun run(block: suspend () -> JSONObject, onOk: (JSONObject) -> Unit) {
        scope.launch {
            busy = true
            error = null
            val r = block()
            busy = false
            if (r.err() != null) error = r.err() else onOk(r)
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(12.dp)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (busy) LoadingBox()
        error?.let { ErrorBanner(it) }

        // ---- 0. 外观（亮色 / 暗色 / 跟随系统） ----
        AppearancePanel()

        // ---- 1. 连接配置（首次必须填一次） ----
        SyncSettingsPanel(onSaved = {
            scope.launch {
                val s = Api.upstreamStatus()
                if (s.err() == null) status = s
            }
        })

        // ---- 1. 同步目标 ----
        Panel(title = "同步目标") {
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                targets.forEach { t ->
                    FilterChip(
                        selected = target == t,
                        onClick = { target = t },
                        label = { Text(t, fontSize = 11.sp) },
                        colors = FilterChipDefaults.filterChipColors(
                            selectedContainerColor = MnemeColors.Accent.copy(alpha = 0.22f),
                            selectedLabelColor = MnemeColors.Accent,
                        ),
                    )
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                AccentButton("立即同步", enabled = !busy, onClick = {
                    run({ Api.sync(target, false) }) { receipt = it }
                })
                GhostButton("仅预演", enabled = !busy, onClick = {
                    run({ Api.sync(target, true) }) { receipt = it }
                })
            }
            receipt?.let { Panel { SectionTitle("同步回执"); Receipt(it) } }
        }

        // ---- 2. 公共基础库（只读上游） ----
        Panel(title = "公共基础库（只读上游）") {
            status?.let { Receipt(it, skipKeys = setOf("error", "targets")) }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                AccentButton("拉取公共基础库", enabled = !busy, onClick = {
                    run({ Api.upstreamPull() }) { plan ->
                        pullPlan = plan
                        // 有冲突就切到「冲突」页逐项裁决；裁决完成前不写任何数据。
                        // 无冲突时 Rust 侧已直接落库，这里只需刷新一次上游状态。
                        if (plan.arrayOrEmpty("conflicts").length() > 0) {
                            onGoConflict()
                        } else {
                            scope.launch {
                                val s = Api.upstreamStatus()
                                if (s.err() == null) status = s
                            }
                        }
                    }
                })
                GhostButton("上游状态", enabled = !busy, onClick = {
                    run({ Api.upstreamStatus() }) { status = it }
                })
            }
            pullPlan?.let { p ->
                // unchanged = 上游 rev 没推进（或本地已是最新），不是「合并成功」
                if (p.optBoolean("unchanged")) {
                    Text(
                        "已是最新：上游 r${p.optInt("upstream_rev")}，无需拉取",
                        color = MnemeColors.Ok,
                        fontSize = 12.sp,
                    )
                    return@let
                }
                val summary = p.optString("summary", "")
                if (summary.isNotBlank()) Text(summary, color = MnemeColors.Warn, fontSize = 12.sp)
                val ap = p.optJSONObject("applied")
                val n = p.arrayOrEmpty("conflicts").length()
                if (ap != null) {
                    Text(
                        "已落库：自动合并 ${ap.optInt("auto_applied")} / 新增 ${ap.optInt("added")} / " +
                            "移除 ${ap.optInt("removed")} / 裁决 ${ap.optInt("conflicts_resolved")} · " +
                            "rev → ${ap.optInt("new_rev")}",
                        color = MnemeColors.Accent,
                        fontSize = 12.sp,
                    )
                } else if (p.optString("apply_error").isNotBlank()) {
                    ErrorBanner("写入失败：" + p.optString("apply_error"))
                } else {
                    Text(
                        if (n > 0) "待裁决 $n 项 → 已切到「冲突」页逐项决定" else "无需裁决，已直接合并",
                        color = MnemeColors.Accent,
                        fontSize = 12.sp,
                    )
                }
            }
        }

        // ---- 3. 导入（含 OCR） ----
        Panel(title = "导入数据") {
            Dim("粘贴文本，或拍照 OCR 识别后回填；导入走 Rust 侧三方合并，并报告 immutable 冲突。")
            OcrInbox(onText = { importText = it })
            MnemeTextField(
                value = importText,
                onValueChange = { importText = it },
                label = "导入内容（CSV / JSON / Bundle）",
                maxLines = 6,
            )
            // 先预览再写：导入是唯一会覆盖本地字段的操作，不能一键到底。
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                GhostButton("预览差异", enabled = !busy && importText.isNotBlank(), onClick = {
                    run({ Api.previewImport(importText, "ocr-or-paste.txt") }) { importPreview = it }
                })
                if (importPreview != null) {
                    GhostButton("清空", onClick = {
                        importPreview = null
                        importResult = null
                    })
                }
                // 自由文本（OCR 识别结果、教材段落）本来就不是 CSV，
                // 走「导入」会报「表头缺少 id 列」；这里直接变成新建草稿。
                GhostButton("按一条药新建", enabled = !busy && importText.isNotBlank(), onClick = {
                    draftOpen = true
                })
            }

            importPreview?.let { pv ->
                val create = pv.optInt("create", 0)
                val update = pv.optInt("update", 0)
                val same = pv.optInt("same", 0)
                Panel {
                    SectionTitle("预览（${pv.optString("format")}）")
                    Text(
                        "新增 $create · 覆盖 $update · 无变化 $same",
                        color = MnemeColors.Text,
                        fontSize = 13.sp,
                    )
                    val warns = pv.arrayOrEmpty("warnings")
                    for (i in 0 until warns.length()) {
                        Text("⚠ ${warns.optString(i)}", color = MnemeColors.Warn, fontSize = 11.sp)
                    }
                    val items = pv.arrayOrEmpty("items")
                    val limit = minOf(items.length(), 20)
                    for (i in 0 until limit) {
                        val o = items.optJSONObject(i) ?: continue
                        val action = o.optString("action")
                        val tag = when (action) {
                            "create" -> "新增"
                            "update" -> "覆盖"
                            else -> "无变化"
                        }
                        val color = when (action) {
                            "create" -> MnemeColors.Ok
                            "update" -> MnemeColors.Warn
                            else -> MnemeColors.TextDim
                        }
                        val changed = o.readable("changed")
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Badge(tag, color)
                            Text(
                                o.optString("name"),
                                color = MnemeColors.Text,
                                fontSize = 12.sp,
                            )
                        }
                        if (changed.isNotBlank()) {
                            Text(
                                "  $changed",
                                color = MnemeColors.TextDim,
                                fontSize = 11.sp,
                            )
                        }
                        val imm = o.readable("immutable")
                        if (imm.isNotBlank()) {
                            Text("  原样字段：$imm", color = MnemeColors.Danger, fontSize = 11.sp)
                        }
                    }
                    if (items.length() > limit) {
                        Dim("…还有 ${items.length() - limit} 条未列出")
                    }
                    if (create + update == 0) {
                        Dim("这份内容和库里一致，导入不会改变任何东西")
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AccentButton(
                        text = if (create + update > 0) "确认写入 ${create + update} 条" else "仍然导入",
                        enabled = !busy,
                        onClick = {
                            run({ Api.importData(importText, "ocr-or-paste.txt", false) }) {
                                importResult = it
                                importPreview = null
                            }
                        },
                    )
                    GhostButton("取消", onClick = { importPreview = null })
                }
            }

            importResult?.let { Panel { SectionTitle("导入结果"); Receipt(it) } }
        }

        // ---- 4. 导出 ----
        Panel(title = "导出数据") {
            val formats = listOf("bundle", "json", "csv", "tsv", "md", "anki", "html")
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                formats.forEach { f ->
                    FilterChip(
                        selected = exportFormat == f,
                        onClick = { exportFormat = f },
                        label = { Text(f, fontSize = 11.sp) },
                        colors = FilterChipDefaults.filterChipColors(
                            selectedContainerColor = MnemeColors.Accent.copy(alpha = 0.22f),
                            selectedLabelColor = MnemeColors.Accent,
                        ),
                    )
                }
            }
            AccentButton("导出 $exportFormat", enabled = !busy, onClick = {
                run({ Api.exportData(exportFormat) }) { exportResult = it }
            })
            exportResult?.let { Panel { SectionTitle("导出回执"); Receipt(it) } }
        }

        Spacer(Modifier.height(16.dp))
    }

    if (draftOpen) {
        PasteFillDialog(
            kind = domainKind,
            initialText = importText,
            afterCreate = {
                // 建完清掉输入框，避免重复建同一条
                importText = ""
                importPreview = null
            },
            onDismiss = { draftOpen = false },
        )
    }
}
