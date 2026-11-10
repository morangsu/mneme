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
import org.json.JSONObject

/**
 * 同步凭据配置。
 *
 * 凭据的存放：**Android Keystore**（`Vault`），不写 `SharedPreferences` 明文、
 * 不写 `sync.json` 明文。落盘配置里只有 `${SECRET:github-private}` 这个引用；
 * 真值在进程内存里，进程退出即消失，下次启动由 `Credentials.bootstrap` 重新注入。
 *
 * 换机 / 撤销：去 GitHub 设置里删掉这个 token，然后在这里「清除凭据」。
 */
@Composable
fun SyncSettingsPanel(onSaved: () -> Unit = {}) {
    val ctx = LocalContext.current
    val sp = remember { ctx.getSharedPreferences("mneme_sync", Context.MODE_PRIVATE) }
    // token 输入框故意**不回填**：回填就等于把明文重新显示在屏幕上，截图/录屏都会带走它
    var token by remember { mutableStateOf("") }
    var owner by remember { mutableStateOf(sp.getString("owner", "morangsu").orEmpty()) }
    var repo by remember { mutableStateOf(sp.getString("repo", "mneme-data-private").orEmpty()) }
    var path by remember { mutableStateOf(sp.getString("path", "data").orEmpty()) }
    var saved by remember { mutableStateOf(Credentials.hasToken(ctx)) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var receipt by remember { mutableStateOf<JSONObject?>(null) }
    val scope = rememberCoroutineScope()

    Panel(title = "连接配置（GitHub 私有库）") {
        Dim("同步只连私有库：每台设备各写自己的事件分片，快照与决策日志同仓可溯源。")
        Dim(
            if (saved) "凭据：已保存在系统密钥库（Android Keystore）。留空表示不修改。"
            else "凭据：未保存。粘贴一次 token 即可，之后只存在密钥库里，不会明文落盘。",
        )
        MnemeTextField(
            token,
            { token = it.trim() },
            if (saved) "新的 personal access token（留空则不修改）"
            else "personal access token（ghp_… / github_pat_…）",
            singleLine = true,
        )
        // 两列并排时长的私有仓库名会被裁掉（mneme-data-private 少了尾巴），
        // 改成各占一行，表单可读性优先于省一屏高度。
        MnemeTextField(owner, { owner = it.trim() }, "owner", singleLine = true)
        MnemeTextField(repo, { repo = it.trim() }, "repo", singleLine = true)
        MnemeTextField(path, { path = it.trim() }, "仓库内路径（留空为根）", singleLine = true)

        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            AccentButton(
                "保存并启用",
                enabled = !busy && (token.isNotBlank() || saved) &&
                    owner.isNotBlank() && repo.isNotBlank(),
            ) {
                scope.launch {
                    busy = true
                    error = null
                    val err = if (token.isNotBlank()) {
                        // 新 token：入箱 → 注入内存 → 重建配置
                        Credentials.saveToken(ctx, token, owner, repo, path)
                    } else {
                        // 没填新 token：只改仓库信息，凭据沿用保险箱里的
                        val r = Api.configureSync(
                            Credentials.buildConfig(ctx, owner, repo, path).toString(),
                        )
                        r.err()
                    }
                    busy = false
                    if (err != null) {
                        error = err
                    } else {
                        sp.edit()
                            .putString("owner", owner)
                            .putString("repo", repo)
                            .putString("path", path)
                            .apply()
                        token = ""
                        saved = Credentials.hasToken(ctx)
                        receipt = JSONObject()
                            .put("ok", true)
                            .put("target", Credentials.GITHUB)
                            .put("token", "已存入 Keystore")
                        onSaved()
                    }
                }
            }
            if (saved) {
                GhostButton("清除凭据", enabled = !busy) {
                    scope.launch {
                        busy = true
                        Credentials.forget(ctx)
                        busy = false
                        saved = false
                        token = ""
                        receipt = null
                        error = null
                    }
                }
            }
        }
        if (busy) LoadingBox()
        error?.let { ErrorBanner(it) }
        receipt?.let { Receipt(it) }
    }
}
