package io.github.morangsu.mneme

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import org.json.JSONObject

/**
 * 空库引导：库里没有任何 schema 时，`views()` 只会返回空数组，界面就停在「加载中…」。
 * 这里列出 APK 内置的领域包，让用户显式选一次；装载后 `SetSchema` 会落进事件日志，
 * 之后可重放、可同步，也可以再用公共库覆盖内容。
 */
@Composable
fun DomainSetupPanel(onInstalled: () -> Unit) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val domains = remember { Bundled.list(ctx) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var receipt by remember { mutableStateOf<JSONObject?>(null) }
    var withSample by remember { mutableStateOf(true) }

    Panel(title = "先选一个领域") {
        Dim("库还是空的。选一个内置领域包建库；内容之后可以从公共库拉取，或自己导入。")
        if (domains.isEmpty()) ErrorBanner("assets/domains/index.json 读取失败")

        domains.forEach { d ->
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        d.name,
                        color = MnemeColors.Text,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        d.id + if (d.sampleLabel != null) " · 含${d.sampleLabel}" else "",
                        color = MnemeColors.TextDim,
                        fontSize = 11.sp,
                    )
                }
                AccentButton("装载", enabled = !busy) {
                    scope.launch {
                        busy = true
                        error = null
                        val toml = runCatching { Bundled.readToml(ctx, d) }.getOrNull()
                        if (toml == null) {
                            error = "读取 ${d.toml} 失败"
                            busy = false
                            return@launch
                        }
                        val r = Api.setSchema(toml)
                        if (r.err() != null) {
                            error = r.err()
                            busy = false
                            return@launch
                        }
                        var done = r
                        // 领域包自带示例数据时顺手导入，省掉「建了库但表还是空的」这一步
                        if (withSample) {
                            val sample = Bundled.readSample(ctx, d)
                            if (sample != null) {
                                val imp = Api.importData(sample, "materia-medica.csv", false)
                                done = JSONObject()
                                    .put("schema", r.optString("schema"))
                                    .put("name", r.optString("name"))
                                    .put("imported", imp.optInt("imported"))
                                    .put("merged", imp.optInt("merged"))
                                    .put("import_error", imp.optString("error", ""))
                            }
                        }
                        receipt = done
                        busy = false
                        onInstalled()
                    }
                }
            }
        }

        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            androidx.compose.material3.Checkbox(
                checked = withSample,
                onCheckedChange = { withSample = it },
            )
            Text("装载时导入该领域的示例数据", color = MnemeColors.TextDim, fontSize = 12.sp)
        }

        if (busy) LoadingBox()
        error?.let { ErrorBanner(it) }
        receipt?.let { Receipt(it) }
    }
}
