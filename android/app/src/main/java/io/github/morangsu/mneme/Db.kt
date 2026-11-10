package io.github.morangsu.mneme

import android.content.Context
import android.provider.Settings
import java.util.UUID
import org.json.JSONObject

/**
 * 本地数据库说明：
 * SQLite / 事件日志 / 快照全部由 Rust 侧 `mneme-store` 管理，APP **不直接开库**，
 * 也不引入 Room。APP 只负责算出目录路径与稳定的 deviceId，
 * 在启动时通过 `Api.openDatabase(path, device)` 把路径交给 Rust，之后所有读写都走 FFI。
 */
object Db {

    /** 数据目录：`filesDir/mneme/data`（Rust 侧会在此放 SQLite / snapshots / events）。 */
    fun databasePath(context: Context): String {
        val dir = java.io.File(context.filesDir, "mneme/data")
        if (!dir.exists()) dir.mkdirs()
        return dir.absolutePath
    }

    /**
     * 稳定的设备标识：优先 ANDROID_ID，缺失/异常时回退随机 UUID，并写入 SharedPreferences。
     * 每台设备写自己的 events/<device>.jsonl 分片，device 必须长期稳定。
     */
    fun deviceId(context: Context): String {
        val sp = context.getSharedPreferences("mneme", Context.MODE_PRIVATE)
        val saved = sp.getString("device_id", null)
        if (!saved.isNullOrBlank()) return saved

        val androidId = try {
            Settings.Secure.getString(context.contentResolver, Settings.Secure.ANDROID_ID)
        } catch (e: Exception) {
            null
        }
        val id = if (!androidId.isNullOrBlank() && androidId != "9774d56d682e549c") {
            "android-$androidId"
        } else {
            "android-${UUID.randomUUID()}"
        }
        sp.edit().putString("device_id", id).apply()
        return id
    }

    /** 初始化 Rust 侧数据库；返回 state 形态的 JSON。失败必须留痕——UI 会静默吞掉它。 */
    suspend fun init(context: Context): JSONObject {
        val r = Api.openDatabase(databasePath(context), deviceId(context))
        if (r.has("error")) {
            android.util.Log.w("mneme", "openDatabase 失败: ${r.optString("error")}")
        }
        return r
    }
}
