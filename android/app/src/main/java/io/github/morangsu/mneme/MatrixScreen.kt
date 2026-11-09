package io.github.morangsu.mneme

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject

private data class ViewDef(val id: String, val label: String)
private data class ColumnDef(val key: String, val label: String, val immutable: Boolean)

/** 表格行模型：表头 / 分组标题 / 数据行，全部交给 LazyColumn 复用。 */
private sealed interface MatrixRow {
    data object Head : MatrixRow
    data class Group(val name: String) : MatrixRow
    data class Data(val cells: List<String>) : MatrixRow
}

/** 视图矩阵：视图下拉 + 分组表格；immutable 列在表头标「原样」。 */
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

    LaunchedEffect(reload) {
        val s = Api.state()
        if (s.err() != null) error = s.err() else stateObj = s
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
        if (defs.isNotEmpty()) selectedId = defs.first().id
    }

    LaunchedEffect(selectedId) {
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
            val rows = buildRows(data, columns)
            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                rows.forEachIndexed { index, row ->
                    item(key = "matrix-$index") {
                        when (row) {
                            is MatrixRow.Head -> HeaderRow(columns)
                            is MatrixRow.Group -> GroupRow(row.name)
                            is MatrixRow.Data -> DataRow(row.cells)
                        }
                    }
                }
            }
        }
        }
    }
}

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

private fun buildRows(data: JSONObject, columns: List<ColumnDef>): List<MatrixRow> {
    val out = mutableListOf<MatrixRow>(MatrixRow.Head)
    val arr = data.optJSONArray("rows") ?: return out
    val groupBy = data.optString("group_by", "")
    var lastGroup: String? = null
    for (i in 0 until arr.length()) {
        val row = arr.optJSONObject(i) ?: continue
        if (groupBy.isNotBlank()) {
            val group = row.optString("_group", "").ifBlank { "未分组" }
            if (group != lastGroup) {
                out += MatrixRow.Group(group)
                lastGroup = group
            }
        }
        out += MatrixRow.Data(columns.map { c -> row.optString(c.key, "") })
    }
    return out
}

@Composable
private fun HeaderRow(columns: List<ColumnDef>) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MnemeColors.PanelAlt, RoundedCornerShape(6.dp))
            .padding(6.dp),
    ) {
        columns.forEach { c ->
            Column(modifier = Modifier.weight(1f).padding(horizontal = 3.dp)) {
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
    }
}

@Composable
private fun GroupRow(name: String) {
    Text(
        "▍$name",
        color = MnemeColors.Accent,
        fontSize = 13.sp,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 8.dp, horizontal = 4.dp),
    )
}

@Composable
private fun DataRow(cells: List<String>) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MnemeColors.Panel, RoundedCornerShape(4.dp))
            .border(0.5.dp, MnemeColors.Divider, RoundedCornerShape(4.dp))
            .padding(6.dp),
    ) {
        cells.forEach { value ->
            Text(
                value.ifBlank { "—" },
                color = MnemeColors.Text,
                fontSize = 11.sp,
                maxLines = 3,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).padding(horizontal = 3.dp),
            )
        }
    }
}
