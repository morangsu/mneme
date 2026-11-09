package io.github.morangsu.mneme

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.delay
import org.json.JSONObject

/**
 * 全文检索页（覆盖层）。
 *
 * 检索逻辑全在 Rust（`mneme_core::search`）：子串 + 同义名 + 拼音首字母，
 * 多词默认 AND、无结果才退化成 OR。这里只负责输入防抖、渲染与跳转，
 * 不重写任何匹配规则 —— 否则两端口径会分叉。
 *
 * 点结果直接进编辑面板：搜到就是为了改/看，不要再多一次「点开详情」。
 */
@Composable
fun SearchScreen(onDismiss: () -> Unit, onOpen: (String) -> Unit) {
    var query by remember { mutableStateOf("") }
    var hits by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var total by remember { mutableStateOf(0) }
    var mode by remember { mutableStateOf("and") }
    var searched by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val focus = LocalFocusManager.current

    // 防抖 220ms：边打字边发 IPC 会把输入法卡顿放大成肉眼可见的迟滞
    LaunchedEffect(query) {
        val q = query.trim()
        if (q.isEmpty()) {
            hits = emptyList()
            total = 0
            searched = false
            error = null
            return@LaunchedEffect
        }
        delay(220)
        val r = Api.search(q, 50)
        if (r.err() != null) {
            error = r.err()
            hits = emptyList()
        } else {
            error = null
            val arr = r.arrayOrEmpty("hits")
            hits = (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }
            total = r.optInt("total", hits.size)
            mode = r.optString("mode", "and")
        }
        searched = true
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(modifier = Modifier.fillMaxSize(), color = MnemeColors.Bg) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(12.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    GhostButton("返回", onClick = onDismiss)
                    OutlinedTextField(
                        value = query,
                        onValueChange = { query = it },
                        label = {
                            Text("搜索药名 / 功效 / 性味 / 别名（可用拼音首字母 mh）", color = MnemeColors.TextDim, fontSize = 12.sp)
                        },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                        textStyle = LocalTextStyle.current.copy(color = MnemeColors.Text, fontSize = 15.sp),
                        colors = OutlinedTextFieldDefaults.colors(
                            focusedBorderColor = MnemeColors.Accent,
                            unfocusedBorderColor = MnemeColors.Divider,
                            focusedLabelColor = MnemeColors.Accent,
                            cursorColor = MnemeColors.Accent,
                        ),
                    )
                }

                error?.let { ErrorBanner(it) }

                if (searched && hits.isNotEmpty()) {
                    Dim(
                        if (mode == "or") "$total 条 · 没有一条同时满足所有词，已改为「命中任意一个词」"
                        else "$total 条 · 每个词都命中",
                    )
                }

                when {
                    query.isBlank() -> Dim("输入药名、功效、性味，或拼音首字母（如 mh → 麻黄）")
                    !searched -> LoadingBox()
                    hits.isEmpty() -> Dim("没有匹配的条目。换个说法：只输入一个词，或用拼音首字母。")
                    else -> LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        hits.forEachIndexed { index, hit ->
                            item(key = "s-$index") {
                                SearchHitCard(hit) {
                                    focus.clearFocus()
                                    onOpen(hit.optString("id"))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SearchHitCard(hit: JSONObject, onClick: () -> Unit) {
    val via = hit.optString("via")
    val label = hit.optString("label")
    val snippet = hit.optString("snippet")
    val group = hit.optString("group")
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .background(MnemeColors.Panel, RoundedCornerShape(8.dp))
            .clickable(onClick = onClick)
            .padding(10.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    hit.optString("name"),
                    color = MnemeColors.Text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                if (via == "pinyin") Badge("拼音", MnemeColors.Warn)
                if (via == "synonym") Badge("别名", MnemeColors.Accent)
            }
            // 字段命中的片段已经带了上下文；药名命中时片段等于名字，不重复显示
            if (snippet.isNotBlank() && snippet != hit.optString("name")) {
                Text(
                    if (label.isNotBlank()) "$label：$snippet" else snippet,
                    color = MnemeColors.TextDim,
                    fontSize = 13.sp,
                )
            }
            if (group.isNotBlank()) {
                Text(group, color = MnemeColors.TextDim, fontSize = 11.sp)
            }
        }
    }
}
