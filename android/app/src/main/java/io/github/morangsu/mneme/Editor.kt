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
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

private data class FieldDef(
    val key: String,
    val label: String,
    val kind: String,
    val immutable: Boolean,
    val required: Boolean,
    val original: String,
)

/**
 * 单元格编辑面板。
 *
 * 只改**可编辑字段**：`immutable` 字段（性味归经、毒性、用量、配伍禁忌、煎法、鉴别）
 * 在界面上直接置灰并标「原样」，点不动 —— 这是领域包里写死的危险区，
 * 改它等于篡改教材原文，需要新建条目而不是就地覆盖。真正的拦截在 Rust 侧
 * （`editEntity` 会再校验一次），UI 的置灰只是提前告知。
 */
@Composable
fun EntityEditorDialog(
    entityId: String,
    onDismiss: () -> Unit,
    onSaved: () -> Unit,
) {
    var title by remember { mutableStateOf("编辑") }
    var subtitle by remember { mutableStateOf("") }
    var fields by remember { mutableStateOf<List<FieldDef>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var result by remember { mutableStateOf<JSONObject?>(null) }
    var busy by remember { mutableStateOf(false) }
    var onlyEditable by remember { mutableStateOf(true) }
    // 详情原始 JSON：复制为新条目时要整份预填
    var detail by remember { mutableStateOf<JSONObject?>(null) }
    var showHistory by remember { mutableStateOf(false) }
    var showCopy by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<String?>(null) }
    val edits = remember { mutableStateMapOf<String, String>() }
    val scope = rememberCoroutineScope()

    // 从磁盘重读：保存后必须刷，否则 original 还是旧值，按钮会一直显示「保存 N 项」、
    // 副标题的 rev 也停在旧值，看起来像没保存成功。
    suspend fun loadDetail() {
        val d = Api.entityDetail(entityId)
        val e = d.err()
        if (e != null) {
            error = e
            return
        }
        detail = d
        title = d.optString("title", "编辑").ifBlank { "编辑" }
        subtitle = "rev ${d.optInt("rev")} · ${d.optString("kind")}"
        val arr = d.arrayOrEmpty("fields")
        val list = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val f = FieldDef(
                key = o.optString("key"),
                label = o.optString("label", o.optString("key")),
                kind = o.optString("kind", "text"),
                immutable = o.optBoolean("immutable"),
                required = o.optBoolean("required"),
                original = o.optString("value"),
            )
            edits[f.key] = f.original
            f
        }
        fields = list
    }

    LaunchedEffect(entityId) { loadDetail() }

    val changedKeys = fields.filter { !it.immutable && edits[it.key].orEmpty() != it.original }.map { it.key }
    val shown = if (onlyEditable) fields.filter { !it.immutable } else fields

    fun save() {
        if (changedKeys.isEmpty() || busy) return
        busy = true
        error = null
        scope.launch {
            val patch = JSONObject()
            changedKeys.forEach { k -> patch.put(k, edits[k].orEmpty()) }
            val r = Api.editEntity(entityId, patch.toString())
            busy = false
            val e = r.err()
            if (e != null) {
                error = e
            } else {
                result = r
                onSaved()
                // 落库后重读：original 与 rev 跟磁盘对齐，按钮自然回到「保存」
                if (!r.optBoolean("noop")) loadDetail()
            }
        }
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
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
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column {
                        Text(
                            title,
                            color = MnemeColors.Text,
                            fontSize = 17.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Text(subtitle, color = MnemeColors.TextDim, fontSize = 11.sp)
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

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    GhostButton(
                        text = if (onlyEditable) "显示全部字段" else "只看可编辑",
                        onClick = { onlyEditable = !onlyEditable },
                    )
                    GhostButton(
                        text = "刷新",
                        enabled = !busy,
                        onClick = {
                            result = null
                            scope.launch { loadDetail() }
                        },
                    )
                }

                error?.let { ErrorBanner(it) }

                notice?.let {
                    Text(it, color = MnemeColors.Ok, fontSize = 12.sp)
                }

                result?.let { r ->
                    val n = r.arrayOrEmpty("changed").length()
                    Text(
                        if (r.optBoolean("noop")) "没有变化，未写入" else "已保存 $n 个字段 · rev ${r.optInt("rev")}",
                        color = MnemeColors.Ok,
                        fontSize = 12.sp,
                    )
                }

                Column(
                    modifier = Modifier
                        .weight(1f)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (fields.isEmpty() && error == null) {
                        Dim("正在读取字段…")
                    }
                    shown.forEach { f ->
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Text(
                                f.label + if (f.required) " *" else "",
                                color = if (f.immutable) MnemeColors.TextDim else MnemeColors.Text,
                                fontSize = 12.sp,
                                fontWeight = FontWeight.Medium,
                                modifier = Modifier.weight(0.32f),
                            )
                            Box(modifier = Modifier.weight(0.68f)) {
                                if (f.immutable) {
                                    Column {
                                        Text(
                                            edits[f.key].orEmpty().ifBlank { "—" },
                                            color = MnemeColors.TextDim,
                                            fontSize = 13.sp,
                                            modifier = Modifier
                                                .fillMaxWidth()
                                                .background(MnemeColors.PanelAlt, RoundedCornerShape(6.dp))
                                                .padding(10.dp),
                                        )
                                        Text("原样字段 · 不可改，需新建条目", color = MnemeColors.Warn, fontSize = 10.sp)
                                    }
                                } else {
                                    MnemeTextField(
                                        value = edits[f.key].orEmpty(),
                                        onValueChange = {
                                            edits[f.key] = it
                                            result = null
                                        },
                                        label = if (f.kind == "tags") "多个用 | 分隔" else f.key,
                                        maxLines = if (f.kind == "tags") 3 else 6,
                                    )
                                }
                            }
                        }
                    }
                }

                // 撤销 / 历史 / 复制为新 / 删除：编辑之外的四条出口
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    GhostButton("撤销", enabled = !busy, onClick = {
                        busy = true
                        notice = null
                        error = null
                        scope.launch {
                            val r = Api.undoEdit(entityId)
                            busy = false
                            val e = r.err()
                            if (e != null) {
                                error = e
                            } else {
                                val n = r.arrayOrEmpty("restored").length()
                                notice = "已撤销最近一次编辑（$n 个字段），rev ${r.optInt("rev")}"
                                result = null
                                loadDetail()
                                onSaved()
                            }
                        }
                    })
                    GhostButton("历史", enabled = !busy, onClick = { showHistory = true })
                    GhostButton("复制为新", enabled = !busy, onClick = { showCopy = true })
                    GhostButton("删除", enabled = !busy, onClick = { confirmDelete = true })
                }

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    AccentButton(
                        text = if (changedKeys.isEmpty()) "保存" else "保存 ${changedKeys.size} 项",
                        enabled = changedKeys.isNotEmpty() && !busy,
                        modifier = Modifier.weight(1f),
                        onClick = { save() },
                    )
                    GhostButton(text = "完成", enabled = !busy, onClick = onDismiss)
                }
            }
        }
    }

    if (showHistory) {
        HistoryDialog(entityId = entityId, onDismiss = { showHistory = false })
    }
    if (showCopy) {
        CreateEntityDialog(
            kind = detail?.optString("kind").orEmpty().ifBlank { "herb" },
            prefill = detail,
            onDismiss = { showCopy = false },
            onSaved = { newId ->
                showCopy = false
                notice = "已新建《$newId》，旧条目已标为被替代"
                onSaved()
            },
        )
    }
    if (confirmDelete) {
        Dialog(
            onDismissRequest = { confirmDelete = false },
            properties = DialogProperties(usePlatformDefaultWidth = true),
        ) {
            Surface(color = MnemeColors.Panel, shape = RoundedCornerShape(12.dp)) {
                Column(
                    modifier = Modifier.padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text("删除「$title」？", color = MnemeColors.Text, fontSize = 15.sp)
                    Text(
                        "只会写墓碑，30 天内在回收站可以恢复。",
                        color = MnemeColors.TextDim,
                        fontSize = 12.sp,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        AccentButton("确认删除", enabled = !busy, onClick = {
                            busy = true
                            scope.launch {
                                val r = Api.deleteEntity(entityId)
                                busy = false
                                val e = r.err()
                                if (e != null) {
                                    error = e
                                    confirmDelete = false
                                } else {
                                    confirmDelete = false
                                    onSaved()
                                    onDismiss()
                                }
                            }
                        })
                        GhostButton("取消", onClick = { confirmDelete = false })
                    }
                }
            }
        }
    }
}
