package io.github.morangsu.mneme

import android.content.Context
import androidx.annotation.WorkerThread
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * 凭据生命周期：**Keystore 持久化 → 注入 Rust 内存 → 迁移历史明文**。
 *
 * 三步的分工必须分清，否则会出现「配置看着对、同步 401」这类难查的问题：
 * 1. Rust 侧只有一个**进程内**注册表，进程被杀就空了 → 每次启动（含后台 Worker）都要重新注入；
 * 2. 注入之后才能重建同步引擎，因为 `${SECRET:x}` 是在建后端时解析的，顺序反了会拿到「未注入」；
 * 3. 历史版本把 token 明文写在两处（`SharedPreferences("mneme_sync")` 和 `sync.json`），
 *    升级后要搬进保险箱并把磁盘上的明文改写成引用——**只搬不删等于没搬**。
 */
object Credentials {

    /** 目前只有 GitHub 私有库一个目标，名字与旧配置里的 `id` 保持一致，迁移不用改引用。 */
    const val GITHUB = "github-private"
    private const val LEGACY_SP = "mneme_sync"
    private val SECRET_KEYS = arrayOf("token", "password", "secret_key", "access_key")

    @Volatile
    private var bootstrapped = false

    /**
     * 幂等启动流程。可被 Activity 与后台 Worker 同时调用：
     * 内存里已有凭据就跳过注入，但仍会检查一次历史明文（代价是一次小文件读）。
     */
    @WorkerThread
    suspend fun bootstrap(ctx: Context): JSONObject = withContext(Dispatchers.IO) {
        val notes = JSONArray()
        // 1. 注入（进程重启后注册表是空的）
        injectAll(ctx, notes)
        // 2. 历史明文迁移（幂等：没有明文时什么都不做）
        //    顺序很重要：**先就地改 sync.json，再用 sp 兜底**。
        //    反过来会把 sync.json 里用户配好的 path/branch 用 sp 里的空值覆盖掉（实测踩过）。
        migrateSyncJson(ctx, notes)
        migratePrefs(ctx, notes)
        // 3. 凭据就位后重建引擎；没配同步时 reloadSync 会静默返回 configured=false
        Api.reloadSync()
        bootstrapped = true
        JSONObject()
            .put("ok", true)
            .put("injected", Vault.names(ctx).size)
            .put("notes", notes)
    }

    /** 保险箱里的凭据全部推进 Rust 内存。解密失败的条目会被 Vault 顺手清掉。 */
    private suspend fun injectAll(ctx: Context, notes: JSONArray) {
        for (name in Vault.names(ctx)) {
            val secret = Vault.get(ctx, name)
            if (secret.isNullOrBlank()) {
                notes.put("凭据 $name 解不开（密钥已失效），已清除，请重新填写")
                continue
            }
            val r = Api.setCredential(name, secret)
            if (r.err() != null) notes.put("凭据 $name 注入失败：${r.err()}")
        }
    }

    /**
     * 迁移 `SharedPreferences("mneme_sync")` 里的明文 token（0.1.0 及更早版本）。
     *
     * 只在「没有 sync.json」时才用 sp 里的字段重建配置；sync.json 已经存在时**绝不重写**，
     * 因为它才是真的配置来源（branch/path 等字段 sp 里根本没有）。
     */
    private suspend fun migratePrefs(ctx: Context, notes: JSONArray) {
        val sp = ctx.getSharedPreferences(LEGACY_SP, Context.MODE_PRIVATE)
        val token = sp.getString("token", "").orEmpty()
        if (token.isBlank()) return
        if (!token.startsWith("\${")) {
            Vault.put(ctx, GITHUB, token)
            Api.setCredential(GITHUB, token)
            notes.put("已把明文 token 从 SharedPreferences 迁入 Keystore")
        }
        val syncJson = File(Db.databasePath(ctx), "sync.json")
        if (!syncJson.exists()) {
            // 只有 sp 有配置（更早的版本）：这时才用 sp 的字段拼一份
            val owner = sp.getString("owner", "").orEmpty().ifBlank { "morangsu" }
            val repo = sp.getString("repo", "").orEmpty().ifBlank { "mneme-data-private" }
            val path = sp.getString("path", "").orEmpty()
            val r = Api.configureSync(buildConfig(ctx, owner, repo, path).toString())
            if (r.err() != null) notes.put("重建同步配置失败：${r.err()}")
        }
        // 明文必须从 SharedPreferences 里删掉，否则等于没迁
        sp.edit().remove("token").apply()
    }

    /**
     * 迁移 `sync.json` 里直接写着真值的凭据字段（手动编辑过配置的情况）。
     *
     * 就地替换：只把凭据字段改成 `${SECRET:名字}`，其余字段（branch/path/exclude…）原样保留，
     * 因为这份文件才是配置的唯一真相，重建反而会丢字段。
     */
    private suspend fun migrateSyncJson(ctx: Context, notes: JSONArray) {
        val file = File(Db.databasePath(ctx), "sync.json")
        if (!file.exists()) return
        val raw = runCatching { file.readText() }.getOrNull() ?: return
        val root = runCatching { JSONObject(raw) }.getOrNull() ?: return
        val targets = root.optJSONArray("targets") ?: return
        var changed = false
        for (i in 0 until targets.length()) {
            val t = targets.optJSONObject(i) ?: continue
            val name = t.optString("id").ifBlank { "target$i" }
            for (key in SECRET_KEYS) {
                val v = t.optString(key, "")
                if (v.isBlank() || v.startsWith("\${")) continue
                Vault.put(ctx, name, v)
                Api.setCredential(name, v)
                t.put(key, "\${SECRET:$name}")
                changed = true
            }
        }
        if (!changed) return
        val r = Api.configureSync(root.toString())
        if (r.err() != null) {
            notes.put("sync.json 明文凭据迁移失败：${r.err()}")
        } else {
            notes.put("已把 sync.json 里的明文凭据迁入 Keystore，配置改为 \${SECRET:…} 引用")
        }
    }

    /**
     * 组装私有库同步配置。`token` 一律写引用，**不写真值**：
     * 真值走 `setCredential`，磁盘上永远看不到。
     */
    fun buildConfig(ctx: Context, owner: String, repo: String, path: String): JSONObject {
        val target = JSONObject()
            .put("kind", "github")
            .put("id", GITHUB)
            .put("token", "\${SECRET:$GITHUB}")
            .put("owner", owner)
            .put("repo", repo)
            .put("branch", "main")
            .put("path", path)
        return JSONObject()
            .put("device_id", Db.deviceId(ctx))
            .put("targets", JSONArray().put(target))
            .put("exclude", JSONArray())
            .put("max_retries", 3)
            .put("upload_snapshots", true)
            .put("keep_snapshots", 30)
    }

    /** 保存 token：入箱 + 注入内存 + 重建配置。返回 null 表示成功，否则是错误文案。 */
    suspend fun saveToken(
        ctx: Context,
        token: String,
        owner: String,
        repo: String,
        path: String,
    ): String? = withContext(Dispatchers.IO) {
        try {
            if (!Vault.put(ctx, GITHUB, token.trim())) return@withContext "密钥库写入失败"
        } catch (e: Exception) {
            // 不回退明文：宁可让用户重试，也不能把安全存储降级成明文
            return@withContext "密钥库不可用：${e.javaClass.simpleName} ${e.message ?: ""}"
        }
        val inj = Api.setCredential(GITHUB, token.trim())
        if (inj.err() != null) return@withContext inj.err()
        val r = Api.configureSync(buildConfig(ctx, owner, repo, path).toString())
        r.err()
    }

    /** 清除凭据：内存与保险箱都要清，否则「清除」后下一次同步照样能跑。 */
    suspend fun forget(ctx: Context) = withContext(Dispatchers.IO) {
        Vault.remove(ctx, GITHUB)
        Api.clearCredential(GITHUB)
    }

    fun hasToken(ctx: Context): Boolean = Vault.has(ctx, GITHUB)

    fun wasBootstrapped(): Boolean = bootstrapped
}
