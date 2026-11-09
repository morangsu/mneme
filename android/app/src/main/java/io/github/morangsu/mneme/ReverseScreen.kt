package io.github.morangsu.mneme

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/**
 * 部分推整体：把「功效:宣肺平喘 禁忌:表虚自汗」这类线索转成 JSON 数组，
 * 交给 Rust 侧倒排索引做 AND 反查（归一化 / 同义表述 / 唯一性收敛全在 Rust）。
 */
@Composable
fun ReverseScreen() {
    var clues by remember { mutableStateOf("功效:宣肺平喘 禁忌:表虚自汗") }
    var result by remember { mutableStateOf<JSONObject?>(null) }
    var loading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    fun runQuery() {
        scope.launch {
            loading = true
            error = null
            val r = Api.reverse(parseClues(clues))
            loading = false
            if (r.err() != null) {
                error = r.err()
                result = null
            } else {
                result = r
            }
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(12.dp)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Panel(title = "部分推整体 · AND 反查") {
            Dim("每行或用空格 / 逗号 / 竖线分隔，格式「字段:值」，例如：功效:宣肺平喘 禁忌:表虚自汗")
            MnemeTextField(
                value = clues,
                onValueChange = { clues = it },
                label = "线索",
                maxLines = 4,
            )
            val preview = parseClues(clues)
            Text(
                "将转换为：$preview",
                color = MnemeColors.TextDim,
                fontSize = 11.sp,
            )
            AccentButton("反查", enabled = !loading && clues.isNotBlank(), onClick = { runQuery() })
        }

        if (loading) LoadingBox()
        error?.let { ErrorBanner(it) }

        val r = result
        if (r != null) {
            val items = r.arrayOrEmpty("items", "hits")
            SectionTitle("命中 ${items.length()} 项")
            if (items.length() == 0) {
                Dim("无命中，试试减少线索或换用同义表述")
            }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                for (i in 0 until items.length()) {
                    val hit = items.optJSONObject(i) ?: continue
                    HitCard(hit)
                }
            }
        }
    }
}

/** 把用户输入切成 ["功效:宣肺平喘","禁忌:表虚自汗"] 形式的 JSON 数组字符串。 */
private fun parseClues(raw: String): String {
    val parts = raw.trim()
        .split(Regex("[\\s,，|；;]+"))
        .map { it.trim() }
        .filter { it.isNotEmpty() }
    val arr = JSONArray()
    parts.forEach { arr.put(it) }
    return arr.toString()
}

@Composable
private fun HitCard(hit: JSONObject) {
    Panel {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                hit.optString("name", hit.optString("id", "?")),
                color = MnemeColors.Text,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f),
            )
            Badge("匹配 ${hit.optDouble("score", 0.0)}", MnemeColors.Ok)
        }
        Dim("分组：${hit.optString("group", "—")} · id：${hit.optString("id", "—")}")
        val summary = hit.optString("summary", "")
        if (summary.isNotBlank()) {
            Text(summary, color = MnemeColors.TextDim, fontSize = 12.sp)
        }
    }
}
