package io.github.morangsu.mneme

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
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
import org.json.JSONObject

/** 分组树里的一项（与 Rust `listGroups` 的 items 同构）。 */
data class GroupItem(
    val id: String,
    val name: String,
    val parent: String,
    val depth: Int,
    val path: String,
    val entities: Int,
)

private fun parseGroups(obj: JSONObject?): List<GroupItem> {
    if (obj == null) return emptyList()
    val arr = obj.arrayOrEmpty("items")
    return (0 until arr.length()).mapNotNull { i ->
        val o = arr.optJSONObject(i) ?: return@mapNotNull null
        GroupItem(
            id = o.optString("id"),
            name = o.optString("name"),
            parent = o.optString("parent"),
            depth = o.optInt("depth"),
            path = o.optString("path"),
            entities = o.optInt("entities"),
        )
    }
}

/**
 * 分组管理：多级树的增删改。
 *
 * 层级上限与「不能挂到自己的下级」的判定都在 Rust 侧（`upsertGroup`），
 * 这里只负责把错误原样显示出来 —— 前端再判一遍只会出现两套规则。
 */
@Composable
fun GroupManagerDialog(
    onDismiss: () -> Unit,
    onChanged: () -> Unit,
) {
    var groups by remember { mutableStateOf<List<GroupItem>>(emptyList()) }
    var unassigned by remember { mutableIntStateOf(0) }
    var error by remember { mutableStateOf<String?>(null) }
    var editing by remember { mutableStateOf<GroupItem?>(null) }
    var creating by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    suspend fun load() {
        val r = Api.listGroups()
        if (r.err() != null) {
            error = r.err()
        } else {
            groups = parseGroups(r)
            unassigned = r.optInt("unassigned", 0)
            error = null
        }
    }

    LaunchedEffect(Unit) { load() }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            color = MnemeColors.Bg,
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier
                .fillMaxWidth(0.94f)
                .fillMaxHeight(0.86f)
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
                        Text("分组", color = MnemeColors.Text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Dim("最多 5 层；点一行可改名 / 改上级 / 删除")
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

                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AccentButton("＋ 新建分组", enabled = !busy, onClick = { creating = true })
                    Dim("未分组 $unassigned 味", modifier = Modifier.padding(top = 12.dp))
                }

                Column(
                    modifier = Modifier
                        .weight(1f)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    if (groups.isEmpty()) {
                        Dim("还没有分组。新建一个「解表药」这样的顶层，再往下建「发散风寒药」。")
                    }
                    groups.forEach { g ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .background(MnemeColors.Panel, RoundedCornerShape(6.dp))
                                .clickable { editing = g }
                                .padding(start = (8 + g.depth * 14).dp, top = 8.dp, bottom = 8.dp, end = 8.dp),
                            verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
                        ) {
                            Text(
                                if (g.depth == 0) "▍${g.name}" else "└ ${g.name}",
                                color = if (g.depth == 0) MnemeColors.Text else MnemeColors.TextDim,
                                fontSize = 13.sp,
                                fontWeight = if (g.depth == 0) FontWeight.SemiBold else FontWeight.Normal,
                                modifier = Modifier.weight(1f),
                            )
                            Dim("${g.entities} 味")
                        }
                    }
                }
            }
        }
    }

    if (creating) {
        GroupEditDialog(
            group = null,
            groups = groups,
            onDismiss = { creating = false },
            onSaved = {
                creating = false
                onChanged()
                scope.launch { load() }
            },
        )
    }
    editing?.let { g ->
        GroupEditDialog(
            group = g,
            groups = groups,
            onDismiss = { editing = null },
            onSaved = {
                editing = null
                onChanged()
                scope.launch { load() }
            },
        )
    }
}

/** 新建 / 重命名 / 改上级 / 删除。`group == null` 表示新建。 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun GroupEditDialog(
    group: GroupItem?,
    groups: List<GroupItem>,
    onDismiss: () -> Unit,
    onSaved: () -> Unit,
) {
    var name by remember { mutableStateOf(group?.name.orEmpty()) }
    var parent by remember { mutableStateOf(group?.parent.orEmpty()) }
    var parentMenu by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    // 不能选自己和自己下级当上级；Rust 侧也会再拦一次
    val parentChoices = groups.filter { g ->
        group == null || (g.id != group.id && !g.path.startsWith(group.path + " / "))
    }
    val parentLabel = parentChoices.firstOrNull { it.id == parent }?.path ?: "（顶层）"

    fun save() {
        if (busy || name.isBlank()) return
        busy = true
        error = null
        scope.launch {
            val r = Api.upsertGroup(group?.id.orEmpty(), name.trim(), parent)
            busy = false
            val e = r.err()
            if (e != null) error = e else onSaved()
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            color = MnemeColors.Bg,
            shape = RoundedCornerShape(14.dp),
            modifier = Modifier.fillMaxWidth(0.94f),
        ) {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Text(
                    if (group == null) "新建分组" else "编辑「${group.name}」",
                    color = MnemeColors.Text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                error?.let { ErrorBanner(it) }

                MnemeTextField(value = name, onValueChange = { name = it }, label = "分组名", singleLine = true)

                ExposedDropdownMenuBox(expanded = parentMenu, onExpandedChange = { parentMenu = it }) {
                    OutlinedTextField(
                        value = parentLabel,
                        onValueChange = {},
                        readOnly = true,
                        label = { Text("上级分组", color = MnemeColors.TextDim, fontSize = 12.sp) },
                        trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(parentMenu) },
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
                    ExposedDropdownMenu(expanded = parentMenu, onDismissRequest = { parentMenu = false }) {
                        DropdownMenuItem(
                            text = { Text("（顶层）", color = MnemeColors.Text) },
                            onClick = {
                                parent = ""
                                parentMenu = false
                            },
                        )
                        parentChoices.forEach { g ->
                            DropdownMenuItem(
                                text = { Text(g.path, color = MnemeColors.Text) },
                                onClick = {
                                    parent = g.id
                                    parentMenu = false
                                },
                            )
                        }
                    }
                }

                if (confirmDelete) {
                    Text(
                        "删除后：子分组上移到上一级，成员变成「未分组」。",
                        color = MnemeColors.Warn,
                        fontSize = 12.sp,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AccentButton("保存", enabled = !busy && name.isNotBlank(), onClick = { save() })
                    if (group != null) {
                        if (!confirmDelete) {
                            GhostButton("删除", enabled = !busy, onClick = { confirmDelete = true })
                        } else {
                            GhostButton("确认删除", enabled = !busy, onClick = {
                                busy = true
                                scope.launch {
                                    val r = Api.deleteGroup(group.id)
                                    busy = false
                                    val e = r.err()
                                    if (e != null) error = e else onSaved()
                                }
                            })
                        }
                    }
                    GhostButton("取消", enabled = !busy, onClick = onDismiss)
                }
            }
        }
    }
}

/**
 * 给实体选分组。列表按树的路径显示，`未分组` 永远在第一个 ——
 * 用户要「移出去」时不该先去找哪个按钮。
 */
@Composable
fun GroupPickerDialog(
    currentGroupId: String,
    onDismiss: () -> Unit,
    onPick: (String) -> Unit,
) {
    var groups by remember { mutableStateOf<List<GroupItem>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        val r = Api.listGroups()
        if (r.err() != null) error = r.err() else groups = parseGroups(r)
    }

    Dialog(onDismissRequest = onDismiss) {
        Surface(color = MnemeColors.Bg, shape = RoundedCornerShape(14.dp)) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 520.dp)
                    .padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text("选择分组", color = MnemeColors.Text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                error?.let { ErrorBanner(it) }
                Column(
                    modifier = Modifier
                        .weight(1f, fill = false)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .background(
                                if (currentGroupId.isBlank()) MnemeColors.PanelAlt else MnemeColors.Panel,
                                RoundedCornerShape(6.dp),
                            )
                            .clickable {
                                if (busy) return@clickable
                                busy = true
                                onPick("")
                            }
                            .padding(10.dp),
                    ) {
                        Text("未分组", color = MnemeColors.Text, fontSize = 13.sp)
                    }
                    groups.forEach { g ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .background(
                                    if (g.id == currentGroupId) MnemeColors.PanelAlt else MnemeColors.Panel,
                                    RoundedCornerShape(6.dp),
                                )
                                .clickable {
                                    if (busy) return@clickable
                                    busy = true
                                    onPick(g.id)
                                }
                                .padding(start = (10 + g.depth * 14).dp, top = 10.dp, bottom = 10.dp, end = 10.dp),
                        ) {
                            Text(
                                if (g.depth == 0) "▍${g.name}" else "└ ${g.name}",
                                color = MnemeColors.Text,
                                fontSize = 13.sp,
                            )
                        }
                    }
                }
                GhostButton("取消", enabled = !busy, onClick = onDismiss)
            }
        }
    }
}
