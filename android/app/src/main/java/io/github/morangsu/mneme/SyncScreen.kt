package io.github.morangsu.mneme

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
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
    var importDry by remember { mutableStateOf(false) }
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

        // ---- 0. 连接配置（首次必须填一次） ----
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
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Checkbox(
                    checked = importDry,
                    onCheckedChange = { importDry = it },
                    colors = CheckboxDefaults.colors(checkedColor = MnemeColors.Accent),
                )
                Dim("仅预演（dry，不写库）")
            }
            AccentButton("导入", enabled = !busy && importText.isNotBlank(), onClick = {
                run({ Api.importData(importText, "ocr-or-paste.txt", importDry) }) { importResult = it }
            })
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
}
