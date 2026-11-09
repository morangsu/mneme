package io.github.morangsu.mneme

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/**
 * 同步凭据配置。
 *
 * Rust 侧只认 `sync.json`（SyncConfig），凭据不硬编码在代码里；这里把用户输入
 * 组装成同样的 JSON，通过 `configureSync` 落盘并重建引擎，不需要重启 APP。
 *
 * token 存在 APP 私有目录（`SharedPreferences` + `filesDir/mneme/data/sync.json`），
 * 不出沙箱、不进日志。换机或撤销时，在 GitHub 设置里删掉这个 token 即可。
 */
@Composable
fun SyncSettingsPanel(onSaved: () -> Unit = {}) {
    val ctx = LocalContext.current
    val sp = remember { ctx.getSharedPreferences("mneme_sync", Context.MODE_PRIVATE) }
    var token by remember { mutableStateOf(sp.getString("token", "").orEmpty()) }
    var owner by remember { mutableStateOf(sp.getString("owner", "morangsu").orEmpty()) }
    var repo by remember { mutableStateOf(sp.getString("repo", "mneme-data-private").orEmpty()) }
    var path by remember { mutableStateOf(sp.getString("path", "data").orEmpty()) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var receipt by remember { mutableStateOf<JSONObject?>(null) }
    val scope = rememberCoroutineScope()

    Panel(title = "连接配置（GitHub 私有库）") {
        Dim("同步只连私有库：每台设备各写自己的事件分片，快照与决策日志同仓可溯源。")
        MnemeTextField(token, { token = it.trim() }, "Personal access token（ghp_… / github_pat_…）", singleLine = true)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            MnemeTextField(
                owner, { owner = it.trim() }, "owner",
                modifier = Modifier.weight(1f), singleLine = true,
            )
            MnemeTextField(
                repo, { repo = it.trim() }, "repo",
                modifier = Modifier.weight(1f), singleLine = true,
            )
        }
        MnemeTextField(path, { path = it.trim() }, "仓库内路径（留空为根）", singleLine = true)

        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            AccentButton("保存并启用", enabled = !busy && token.isNotBlank() && owner.isNotBlank() && repo.isNotBlank()) {
                scope.launch {
                    busy = true
                    error = null
                    val cfg = JSONObject()
                        .put("device_id", Db.deviceId(ctx))
                        .put(
                            "targets",
                            JSONArray().put(
                                JSONObject()
                                    .put("kind", "github")
                                    .put("id", "github-private")
                                    .put("token", token)
                                    .put("owner", owner)
                                    .put("repo", repo)
                                    .put("branch", "main")
                                    .put("path", path),
                            ),
                        )
                        .put("exclude", JSONArray())
                        .put("max_retries", 3)
                        .put("upload_snapshots", true)
                        .put("keep_snapshots", 30)
                    val r = Api.configureSync(cfg.toString())
                    busy = false
                    if (r.err() != null) {
                        error = r.err()
                    } else {
                        sp.edit()
                            .putString("token", token)
                            .putString("owner", owner)
                            .putString("repo", repo)
                            .putString("path", path)
                            .apply()
                        receipt = r
                        onSaved()
                    }
                }
            }
            if (token.isNotBlank()) {
                GhostButton("清除凭据", enabled = !busy) {
                    token = ""
                    sp.edit().remove("token").apply()
                    receipt = null
                }
            }
        }
        if (busy) LoadingBox()
        error?.let { ErrorBanner(it) }
        receipt?.let { Receipt(it) }
    }
}
