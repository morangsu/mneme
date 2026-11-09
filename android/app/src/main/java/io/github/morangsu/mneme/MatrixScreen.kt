package io.github.morangsu.mneme

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject

private data class ViewDef(val id: String, val label: String)
private data class ColumnDef(val key: String, val label: String, val immutable: Boolean)

/**
 * 表格行模型。
 *
 * `Group` 带 `path`：多级分组靠路径识别（`解表药 / 发散风寒药`），
 * 折叠状态也按路径前缀存 —— 折叠「解表药」会把它所有下级一起收起。
 */
private sealed interface MatrixRow {
    data object Head : MatrixRow
    data class Group(val path: List<String>, val count: Int) : MatrixRow {
        val depth: Int get() = path.size - 1
        val key: String get() = path.joinToString("\u0001")
        val name: String get() = path.last()
    }

    data class Data(val id: String, val cells: List<String>) : MatrixRow
}

/** 矩阵页：视图下拉 + 多级分组表格/卡片；immutable 列在表头标「原样」。 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MatrixScreen() {
    var stateObj by remember { mutableStateOf<JSONObject?>(null) }
    var viewDefs by remember { mutableStateOf<List<ViewDef>>(emptyList()) }
    var selectedId by remember { mutableStateOf<String?>(null) }
    var viewData by remember { mutableStateOf<JSONObject?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var expanded by remember { mutableStateOf(false) }
    // 装载领域包后 +1，触发重新读 state / views（否则会一直停在空库状态）
    var reload by remember { mutableIntStateOf(0) }
    var loaded by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }
    var batchPaste by remember { mutableStateOf(false) }
    var recycle by remember { mutableStateOf(false) }
    var groupMgr by remember { mutableStateOf(false) }
    var searching by remember { mutableStateOf(false) }
    var kind by remember { mutableStateOf("herb") }
    // 手机屏一行放不下 4 列长文本：默认卡片（全文换行），要横向比较再切表格
    var cardMode by remember { mutableStateOf(true) }
    // 折叠的分组路径（"\u0001" 连接）
    var collapsed by remember { mutableStateOf(setOf<String>()) }

    LaunchedEffect(reload) {
        val s = Api.state()
        if (s.err() != null) error = s.err() else {
            stateObj = s
            if (s.optString("kind").isNotBlank()) kind = s.optString("kind")
        }
        val v = Api.views()
        if (v.err() != null) {
            error = v.err()
            loaded = true
            return@LaunchedEffect
        }
        val arr = v.arrayOrEmpty("views")
        val defs = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            ViewDef(o.optString("id"), o.optString("label", o.optString("id")))
        }
        viewDefs = defs
        loaded = true
        if (defs.isNotEmpty() && selectedId == null) selectedId = defs.first().id
    }

    LaunchedEffect(selectedId, reload) {
        val id = selectedId ?: return@LaunchedEffect
        val r = Api.view(id)
        if (r.err() != null) {
            error = r.err()
        } else {
            viewData = r
            error = null
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Panel {
            val s = stateObj
            if (s == null) {
                Dim("正在读取状态…")
            } else {
                Text(
                    "领域 ${s.optString("schema")} · 实体 ${s.optInt("entities")} · 分组 ${s.optInt("groups")} · " +
                        "卡片 ${s.optInt("cards")} · 事件 ${s.optInt("events")} · rev ${s.optInt("rev")}",
                    color = MnemeColors.Text,
                    fontSize = 13.sp,
                )
                if (s.optInt("entities") > 0) {
                    Text("点任意一行可编辑；「原样」字段不可改", color = MnemeColors.TextDim, fontSize = 11.sp)
                }
                // 四个按钮在 1080px 屏放不下一行（「回收站」会被裁掉），横向可滑
                Row(
                    modifier = Modifier.horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    GhostButton("检索", onClick = { searching = true })
                    GhostButton("＋ 新建", onClick = { creating = true })
                    GhostButton("批量粘贴", onClick = { batchPaste = true })
                    GhostButton("分组", onClick = { groupMgr = true })
                    GhostButton(
                        "回收站 (${s.optInt("recycle")})",
                        onClick = { recycle = true },
                    )
                }
            }
        }

        if (loaded && viewDefs.isEmpty()) {
            DomainSetupPanel(onInstalled = { reload++ })
        } else {
            ExposedDropdownMenuBox(
                expanded = expanded,
                onExpandedChange = { expanded = it },
            ) {
                OutlinedTextField(
                    value = viewDefs.firstOrNull { it.id == selectedId }?.label ?: "选择视图",
                    onValueChange = {},
                    readOnly = true,
                    label = { Text("视图", color = MnemeColors.TextDim, fontSize = 12.sp) },
                    trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(expanded) },
                    modifier = Modifier
                        .menuAnchor()
                        .fillMaxWidth(),
                    textStyle = LocalTextStyle.current.copy(color = MnemeColors.Text, fontSize = 14.sp),
                    colors = OutlinedTextFieldDefaults.colors(
                        focusedBorderColor = MnemeColors.Accent,
                        unfocusedBorderColor = MnemeColors.Divider,
                        focusedLabelColor = MnemeColors.Accent,
                        cursorColor = MnemeColors.Accent,
                    ),
                )
                ExposedDropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
                    viewDefs.forEach { def ->
                        DropdownMenuItem(
                            text = { Text(def.label, color = MnemeColors.Text) },
                            onClick = {
                                selectedId = def.id
                                expanded = false
                            },
                        )
                    }
                }
            }

            error?.let { ErrorBanner(it) }

            val data = viewData
            if (data == null) {
                LoadingBox()
            } else {
                val columns = parseColumns(data.optJSONArray("columns"))
                val rows = buildRows(data, collapsed)
                val groupCount = data.optInt("group_count", 0)

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    GhostButton(
                        text = if (cardMode) "切到表格" else "切到卡片",
                        onClick = { cardMode = !cardMode },
                    )
                    if (collapsed.isNotEmpty()) {
                        GhostButton("展开全部", onClick = { collapsed = emptySet() })
                    }
                    Dim(
                        if (groupCount > 0) "$groupCount 个分组 · ${columns.size} 列"
                        else "${columns.size} 列",
                    )
                }

                if (cardMode) {
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        rows.forEachIndexed { index, row ->
                            item(key = "m-$index") {
                                when (row) {
                                    is MatrixRow.Group -> GroupHeader(row, collapsed) {
                                        collapsed = toggleCollapsed(collapsed, row.key)
                                    }

                                    is MatrixRow.Data -> EntityCard(row.cells, columns) { editing = row.id }
                                    MatrixRow.Head -> Unit
                                }
                            }
                        }
                    }
                } else {
                    // 表格：行与表头共用一个横向滚动状态，列才对得齐
                    val hScroll = rememberScrollState()
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        item(key = "m-head") {
                            Row(
                                modifier = Modifier
                                    .horizontalScroll(hScroll)
                                    .background(MnemeColors.PanelAlt, RoundedCornerShape(6.dp))
                                    .padding(6.dp),
                            ) {
                                columns.forEachIndexed { i, c -> HeaderCell(c, i) }
                            }
                        }
                        rows.forEachIndexed { index, row ->
                            if (row is MatrixRow.Group) {
                                item(key = "m-$index") {
                                    GroupHeader(row, collapsed) {
                                        collapsed = toggleCollapsed(collapsed, row.key)
                                    }
                                }
                            } else if (row is MatrixRow.Data) {
                                item(key = "m-$index") {
                                    Row(
                                        modifier = Modifier
                                            .horizontalScroll(hScroll)
                                            .background(MnemeColors.Panel, RoundedCornerShape(4.dp))
                                            .border(0.5.dp, MnemeColors.Divider, RoundedCornerShape(4.dp))
                                            .clickable { editing = row.id }
                                            .padding(6.dp),
                                    ) {
                                        row.cells.forEachIndexed { i, value ->
                                            Text(
                                                value.ifBlank { "—" },
                                                color = MnemeColors.Text,
                                                fontSize = 11.sp,
                                                maxLines = 5,
                                                overflow = TextOverflow.Ellipsis,
                                                modifier = Modifier
                                                    .width(columnWidth(i))
                                                    .padding(horizontal = 3.dp),
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if (searching) {
        // 检索页点结果直接进编辑面板：搜到就是为了看/改，不再多一次跳转
        SearchScreen(
            onDismiss = { searching = false },
            onOpen = {
                searching = false
                editing = it
            },
        )
    }
    editing?.let { id ->
        EntityEditorDialog(
            entityId = id,
            onDismiss = { editing = null },
            onSaved = { reload++ },
        )
    }
    if (creating) {
        CreateEntityDialog(
            kind = kind,
            prefill = null,
            onDismiss = { creating = false },
            onSaved = {
                creating = false
                reload++
            },
        )
    }
    if (batchPaste) {
        PasteFillDialog(
            kind = kind,
            afterCreate = { reload++ },
            onDismiss = { batchPaste = false },
        )
    }
    if (groupMgr) {
        GroupManagerDialog(
            onDismiss = { groupMgr = false },
            onChanged = { reload++ },
        )
    }
    if (recycle) {
        RecycleBinDialog(
            onDismiss = { recycle = false },
            onChanged = { reload++ },
        )
    }
}

/** 列宽：首列窄一点（药名短），其余给宽 —— 长文本靠换行而不是省略号。 */
private fun columnWidth(index: Int) = if (index == 0) 108.dp else 176.dp

private fun toggleCollapsed(current: Set<String>, key: String): Set<String> =
    if (key in current) current - key else current + key

private fun parseColumns(arr: JSONArray?): List<ColumnDef> {
    if (arr == null) return emptyList()
    return (0 until arr.length()).mapNotNull { i ->
        val o = arr.optJSONObject(i) ?: return@mapNotNull null
        ColumnDef(
            key = o.optString("key"),
            label = o.optString("label", o.optString("key")),
            immutable = o.optBoolean("immutable"),
        )
    }
}

/**
 * 把扁平行列表展开成「分组头 + 数据行」，并按折叠状态过滤。
 *
 * 层级来自后端下发的 `_group_path`（`["解表药","发散风寒药"]`）；
 * 没挂分组的行归到「未分组」，这样它也能被折叠，而不是散在最上面。
 */
private fun buildRows(data: JSONObject, collapsed: Set<String>): List<MatrixRow> {
    val out = mutableListOf<MatrixRow>()
    val arr = data.optJSONArray("rows") ?: return out
    val groupBy = data.optString("group_by", "")
    if (groupBy.isBlank()) {
        for (i in 0 until arr.length()) {
            val row = arr.optJSONObject(i) ?: continue
            out += MatrixRow.Data(row.optString("_id"), cellsOf(row, data))
        }
        return out
    }

    // 先数每层有多少条，分组头才能显示「N 味」
    val paths = (0 until arr.length()).map { i -> pathOf(arr.optJSONObject(i)) }
    val counts = mutableMapOf<String, Int>()
    paths.forEach { p ->
        for (n in 1..p.size) {
            val key = p.take(n).joinToString("\u0001")
            counts[key] = (counts[key] ?: 0) + 1
        }
    }

    val emitted = mutableSetOf<String>()
    for (i in 0 until arr.length()) {
        val row = arr.optJSONObject(i) ?: continue
        val path = paths[i]
        var hidden = false
        for (n in 1..path.size) {
            val key = path.take(n).joinToString("\u0001")
            // 先发自己的分组头，再判断「这一层是否被折叠」——
            // 顺序反了会把被折叠的分组头也一起藏掉，用户只能点「展开全部」才能回来。
            if (!hidden && emitted.add(key)) {
                out += MatrixRow.Group(path.take(n), counts[key] ?: 0)
            }
            if (key in collapsed) hidden = true
        }
        if (!hidden) {
            out += MatrixRow.Data(row.optString("_id"), cellsOf(row, data))
        }
    }
    return out
}

private fun pathOf(row: JSONObject?): List<String> {
    if (row == null) return listOf("未分组")
    val arr = row.optJSONArray("_group_path") ?: return listOf("未分组")
    val names = (0 until arr.length()).mapNotNull { arr.optString(it, "").ifBlank { null } }
    return if (names.isEmpty()) listOf("未分组") else names
}

private fun cellsOf(row: JSONObject, data: JSONObject): List<String> {
    val cols = data.optJSONArray("columns") ?: return emptyList()
    return (0 until cols.length()).map { i ->
        val key = cols.optJSONObject(i)?.optString("key").orEmpty()
        row.optString(key, "")
    }
}

@Composable
private fun HeaderCell(c: ColumnDef, index: Int) {
    Column(modifier = Modifier.width(columnWidth(index)).padding(horizontal = 3.dp)) {
        Text(
            c.label,
            color = MnemeColors.TextDim,
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (c.immutable) Text("原样", color = MnemeColors.Warn, fontSize = 9.sp)
    }
}

/** 分组头：可折叠，缩进体现层级。 */
@Composable
private fun GroupHeader(row: MatrixRow.Group, collapsed: Set<String>, onToggle: () -> Unit) {
    val isCollapsed = row.key in collapsed
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onToggle() }
            .padding(start = (4 + row.depth * 14).dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            if (isCollapsed) "▸" else "▾",
            color = MnemeColors.Accent,
            fontSize = 12.sp,
        )
        Spacer(Modifier.width(4.dp))
        Text(
            row.name,
            color = if (row.depth == 0) MnemeColors.Text else MnemeColors.Accent,
            fontSize = if (row.depth == 0) 14.sp else 12.sp,
            fontWeight = if (row.depth == 0) FontWeight.SemiBold else FontWeight.Medium,
        )
        Spacer(Modifier.width(6.dp))
        Text("${row.count}", color = MnemeColors.TextDim, fontSize = 10.sp)
    }
}

/**
 * 卡片模式：每味药一张卡，字段**完整换行**，不再截断。
 *
 * 表格适合横向比较，但手机上 4 列长文本必然被省略号吃掉 ——
 * 这正是「内容显示不全」的根源，所以默认给卡片。
 */
@Composable
private fun EntityCard(cells: List<String>, columns: List<ColumnDef>, onClick: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(MnemeColors.Panel, RoundedCornerShape(8.dp))
            .border(0.5.dp, MnemeColors.Divider, RoundedCornerShape(8.dp))
            .clickable { onClick() }
            .padding(10.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        val title = cells.firstOrNull().orEmpty()
        Text(
            title.ifBlank { "（无名称）" },
            color = MnemeColors.Text,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
        )
        columns.drop(1).forEachIndexed { i, c ->
            val value = cells.getOrNull(i + 1).orEmpty()
            if (value.isNotBlank()) {
                Row {
                    Text(
                        c.label + if (c.immutable) "（原样）" else "",
                        color = MnemeColors.TextDim,
                        fontSize = 11.sp,
                        modifier = Modifier.width(72.dp),
                    )
                    Text(
                        value,
                        color = MnemeColors.Text,
                        fontSize = 12.sp,
                        lineHeight = 17.sp,
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        }
    }
}
