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
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

private data class TplField(
    val key: String,
    val label: String,
    val kind: String,
    val immutable: Boolean,
    val required: Boolean,
    val value: String,
)

/**
 * 新建条目。
 *
 * - 空白新建：字段定义来自 `entityTemplate(kind)`；
 * - 复制为新（`prefill` 非空）：预填原条目的全部字段，并自动写 `supersedes`，
 *   这是「原样字段抄错了」的唯一正路 —— 不改原条目，另建一条并指回去。
 *
 * 与编辑面板的关键差别：**这里 immutable 字段是可以填的**（新条目本来就没有原文可保护），
 * 只提示"填完就不可再改"。
 */
@Composable
fun CreateEntityDialog(
    kind: String,
    prefill: JSONObject?,
    onDismiss: () -> Unit,
    onSaved: (String) -> Unit,
) {
    var title by remember { mutableStateOf(if (prefill != null) "复制为新条目" else "新建条目") }
    var fields by remember { mutableStateOf<List<TplField>>(emptyList()) }
    var idHint by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var savedId by remember { mutableStateOf<String?>(null) }
    var pasteOpen by remember { mutableStateOf(false) }
    // 主键字段由 schema 决定，不能在 UI 里写死 "name"
    var primaryKey by remember { mutableStateOf("name") }
    // 查重结果：(id, 名字, 原因)
    var dups by remember { mutableStateOf<List<Triple<String, String, String>>>(emptyList()) }
    val edits = remember { mutableStateMapOf<String, String>() }
    val scope = rememberCoroutineScope()

    LaunchedEffect(kind, prefill) {
        val src = prefill
        val d = if (src != null) src else Api.entityTemplate(kind)
        val e = d.err()
        if (e != null) {
            error = e
            return@LaunchedEffect
        }
        val arr = d.arrayOrEmpty("fields")
        val primary = d.optString("primary", "name")
        primaryKey = primary
        var primaryValue = ""
        fields = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val f = TplField(
                key = o.optString("key"),
                label = o.optString("label", o.optString("key")),
                kind = o.optString("kind", "text"),
                immutable = o.optBoolean("immutable"),
                required = o.optBoolean("required"),
                value = o.optString("value"),
            )
            edits[f.key] = f.value
            if (f.key == primary) primaryValue = f.value
            f
        }
        // 复制为新：默认名加后缀，避免直接撞主键；用户可改
        if (src != null) {
            idHint = if (primaryValue.isNotBlank()) "$primaryValue（修订）" else ""
            edits[primary] = idHint
        }
        title = if (src != null) "复制为新条目" else "新建条目"
    }

    // 边填边查重：输入停 350ms 再问一次，避免每敲一个字都过一遍 FFI。
    // 只提示不拦截 —— 「麻黄」和「麻黄（修订）」可能本来就该并存。
    LaunchedEffect(edits[primaryKey], primaryKey, kind) {
        val name = edits[primaryKey].orEmpty().trim()
        if (name.isEmpty()) {
            dups = emptyList()
            return@LaunchedEffect
        }
        delay(350)
        val body = JSONObject().put(primaryKey, name)
        val r = Api.checkDuplicate(kind, body.toString(), prefill?.optString("id").orEmpty())
        if (r.err() == null) {
            val arr = r.arrayOrEmpty("items")
            dups = (0 until arr.length()).mapNotNull { i ->
                val o = arr.optJSONObject(i) ?: return@mapNotNull null
                Triple(o.optString("id"), o.optString("name"), o.optString("reason"))
            }
        }
    }

    fun save() {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            val patch = JSONObject()
            fields.forEach { f ->
                val v = edits[f.key].orEmpty()
                if (v.isNotBlank()) patch.put(f.key, v)
            }
            // 复制为新：把来源 id 写进 supersedes（旧条目会被矩阵自动隐藏）
            if (prefill != null) {
                val srcId = prefill.optString("id")
                if (srcId.isNotBlank() && fields.any { it.key == "supersedes" }) {
                    patch.put("supersedes", srcId)
                }
            }
            val r = Api.createEntity(kind, patch.toString(), idHint.trim())
            busy = false
            val e = r.err()
            if (e != null) {
                error = e
            } else {
                savedId = r.optString("id")
                onSaved(savedId!!)
            }
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            color = MnemeColors.Bg,
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier
                .fillMaxWidth(0.94f)
                .fillMaxHeight(0.92f)
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
                        Text(title, color = MnemeColors.Text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Text(
                            if (prefill != null) "原样字段可以改，但保存后同样不可再改" else "原样字段填完即锁定",
                            color = MnemeColors.TextDim,
                            fontSize = 11.sp,
                        )
                    }
                    Text(
                        "关闭",
                        color = MnemeColors.Accent,
                        fontSize = 13.sp,
                        modifier = Modifier.clickable { onDismiss() }.padding(6.dp),
                    )
                }

                error?.let { ErrorBanner(it) }
                savedId?.let {
                    Text("已新建：$it", color = MnemeColors.Ok, fontSize = 12.sp)
                }

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    GhostButton("粘贴填充", onClick = { pasteOpen = true })
                    Dim("把教材那一段粘进来，自动对上字段", modifier = Modifier.padding(top = 12.dp))
                }

                if (dups.isNotEmpty()) {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .background(MnemeColors.Warn.copy(alpha = 0.10f), RoundedCornerShape(8.dp))
                            .padding(8.dp),
                        verticalArrangement = Arrangement.spacedBy(3.dp),
                    ) {
                        Text("⚠ 可能重复", color = MnemeColors.Warn, fontSize = 12.sp)
                        dups.forEach { (id, name, reason) ->
                            Text(
                                "· $name（$reason）· 点此处可打开它",
                                color = MnemeColors.TextDim,
                                fontSize = 11.sp,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable { onSaved(id) }
                                    .padding(vertical = 2.dp),
                            )
                        }
                        Text(
                            "确认不是同一味就继续保存；是的话先关掉这个窗口去改那一条。",
                            color = MnemeColors.TextDim,
                            fontSize = 11.sp,
                        )
                    }
                }

                Column(
                    modifier = Modifier
                        .weight(1f)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (fields.isEmpty() && error == null) Dim("正在读取字段定义…")
                    fields.forEach { f ->
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Column(modifier = Modifier.weight(0.32f)) {
                                Text(
                                    f.label + if (f.required) " *" else "",
                                    color = MnemeColors.Text,
                                    fontSize = 12.sp,
                                    fontWeight = FontWeight.Medium,
                                )
                                if (f.immutable) {
                                    Text("原样·锁定", color = MnemeColors.Warn, fontSize = 9.sp)
                                }
                            }
                            Box(modifier = Modifier.weight(0.68f)) {
                                MnemeTextField(
                                    value = edits[f.key].orEmpty(),
                                    onValueChange = { edits[f.key] = it },
                                    label = if (f.kind == "tags") "多个用 | 分隔" else f.key,
                                    maxLines = if (f.kind == "tags") 3 else 4,
                                )
                            }
                        }
                    }
                    MnemeTextField(
                        value = idHint,
                        onValueChange = { idHint = it },
                        label = "内部 id（可留空，默认用主键）",
                        singleLine = true,
                    )
                }

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AccentButton(
                        text = if (savedId != null) "已保存" else "保存",
                        enabled = !busy && savedId == null,
                        modifier = Modifier.weight(1f),
                        onClick = { save() },
                    )
                    GhostButton(text = if (savedId != null) "完成" else "取消", onClick = onDismiss)
                }
            }
        }
    }

    if (pasteOpen) {
        PasteFillDialog(
            kind = kind,
            onDismiss = { pasteOpen = false },
            onApply = { values ->
                // 只覆盖认出来的字段，其余保持用户已填的内容
                values.forEach { (k, v) -> edits[k] = v }
                pasteOpen = false
            },
        )
    }
}

/** 单条实体的字段级变更时间线（只读）。 */
@Composable
fun HistoryDialog(entityId: String, onDismiss: () -> Unit) {
    var items by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var loaded by remember { mutableStateOf(false) }

    LaunchedEffect(entityId) {
        val r = Api.entityHistory(entityId)
        val e = r.err()
        if (e != null) error = e else {
            val arr = r.arrayOrEmpty("items")
            items = (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }.reversed()
        }
        loaded = true
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            color = MnemeColors.Bg,
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier
                .fillMaxWidth(0.94f)
                .fillMaxHeight(0.8f)
                .border(1.dp, MnemeColors.Divider, RoundedCornerShape(14.dp)),
        ) {
            Column(
                modifier = Modifier.fillMaxSize().padding(14.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Text("变更历史", color = MnemeColors.Text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    Text("关闭", color = MnemeColors.Accent, fontSize = 13.sp,
                        modifier = Modifier.clickable { onDismiss() }.padding(6.dp))
                }
                error?.let { ErrorBanner(it) }
                Column(
                    modifier = Modifier.weight(1f).verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (!loaded) Dim("读取中…")
                    if (loaded && items.isEmpty() && error == null) Dim("没有变更记录")
                    items.forEach { o ->
                        val action = o.optString("action")
                        Panel {
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    Badge(actionLabel(action), actionColor(action))
                                    Text(
                                        fmtTime(o.optLong("ts")),
                                        color = MnemeColors.TextDim,
                                        fontSize = 11.sp,
                                    )
                                }
                                Text(
                                    "${o.optString("device")} · rev ${o.optInt("rev")}",
                                    color = MnemeColors.TextDim,
                                    fontSize = 10.sp,
                                )
                            }
                            val ch = o.arrayOrEmpty("changes")
                            if (ch.length() == 0) {
                                Dim(if (action == "create") "新建条目" else "无字段差异")
                            }
                            for (i in 0 until ch.length()) {
                                val c = ch.optJSONObject(i) ?: continue
                                Text(
                                    "${c.optString("key")}：${c.optString("before").ifBlank { "（空）" }} → " +
                                        c.optString("after").ifBlank { "（空）" },
                                    color = MnemeColors.Text,
                                    fontSize = 11.sp,
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

/** 回收站：列出墓碑条目，可逐条恢复。 */
@Composable
fun RecycleBinDialog(onDismiss: () -> Unit, onChanged: () -> Unit) {
    var items by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var loaded by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    suspend fun reload() {
        val r = Api.listDeleted()
        val e = r.err()
        if (e != null) {
            error = e
        } else {
            val arr = r.arrayOrEmpty("items")
            items = (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }
            error = null
        }
        loaded = true
    }

    LaunchedEffect(Unit) { reload() }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            color = MnemeColors.Bg,
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier
                .fillMaxWidth(0.94f)
                .fillMaxHeight(0.7f)
                .border(1.dp, MnemeColors.Divider, RoundedCornerShape(14.dp)),
        ) {
            Column(
                modifier = Modifier.fillMaxSize().padding(14.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Column {
                        Text("回收站", color = MnemeColors.Text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Text("删除是墓碑，可恢复；30 天后可清理", color = MnemeColors.TextDim, fontSize = 11.sp)
                    }
                    Text("关闭", color = MnemeColors.Accent, fontSize = 13.sp,
                        modifier = Modifier.clickable { onDismiss() }.padding(6.dp))
                }
                error?.let { ErrorBanner(it) }
                Column(
                    modifier = Modifier.weight(1f).verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (!loaded) Dim("读取中…")
                    if (loaded && items.isEmpty() && error == null) Dim("回收站是空的")
                    items.forEach { o ->
                        val id = o.optString("id")
                        Panel {
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column {
                                    Text(o.optString("name", id), color = MnemeColors.Text, fontSize = 14.sp)
                                    Text(
                                        "删除于 ${fmtTime(o.optLong("deleted_at"))} · ${o.optString("device")}",
                                        color = MnemeColors.TextDim,
                                        fontSize = 10.sp,
                                    )
                                }
                                GhostButton("恢复", enabled = !busy, onClick = {
                                    busy = true
                                    scope.launch {
                                        val r = Api.restoreEntity(id)
                                        busy = false
                                        val e = r.err()
                                        if (e != null) error = e else {
                                            onChanged()
                                            reload()
                                        }
                                    }
                                })
                            }
                        }
                    }
                }
            }
        }
    }
}

private fun actionLabel(action: String): String = when (action) {
    "create" -> "新建"
    "update" -> "修改"
    "delete" -> "删除"
    "restore" -> "恢复"
    else -> action
}

private fun actionColor(action: String) = when (action) {
    "create" -> MnemeColors.Ok
    "delete" -> MnemeColors.Danger
    "restore" -> MnemeColors.Accent
    else -> MnemeColors.Warn
}

private fun fmtTime(ts: Long): String =
    if (ts <= 0) "—" else SimpleDateFormat("MM-dd HH:mm", Locale.getDefault()).format(Date(ts))
