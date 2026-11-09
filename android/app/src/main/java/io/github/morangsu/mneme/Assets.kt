package io.github.morangsu.mneme

import android.content.Context
import org.json.JSONObject

/**
 * 内置领域包（APK assets/domains）。
 *
 * 为什么要内置：APP 冷启动时库里没有任何 schema，`views()` 只能返回空列表，
 * 界面就停在「加载中…」。领域包必须由用户主动选一次（相当于"建库"），
 * 落成 `SetSchema` 事件后可重放、可同步，不影响以后从公共库拉取。
 *
 * 内容与 https://github.com/morangsu/mneme-data-public 一致，构建时由
 * `android/sync-assets.cmd` 从 domains/ 同步过来，避免两处手改。
 */
data class BundledDomain(
    val id: String,
    val name: String,
    val toml: String,
    val sample: String?,
    val sampleLabel: String?,
)

object Bundled {

    private const val DIR = "domains"

    fun list(context: Context): List<BundledDomain> = try {
        val raw = context.assets.open("$DIR/index.json").bufferedReader().use { it.readText() }
        val arr = JSONObject(raw).optJSONArray("domains") ?: return emptyList()
        (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val toml = o.optString("toml")
            if (toml.isBlank()) return@mapNotNull null
            BundledDomain(
                id = o.optString("id", toml.removeSuffix(".toml")),
                name = o.optString("name", o.optString("id")),
                toml = toml,
                sample = o.optString("sample").ifBlank { null },
                sampleLabel = o.optString("sample_label").ifBlank { null },
            )
        }
    } catch (e: Exception) {
        emptyList()
    }

    fun readToml(context: Context, d: BundledDomain): String =
        context.assets.open("$DIR/${d.toml}").bufferedReader().use { it.readText() }

    fun readSample(context: Context, d: BundledDomain): String? {
        val name = d.sample ?: return null
        return try {
            context.assets.open("$DIR/$name").bufferedReader().use { it.readText() }
        } catch (e: Exception) {
            null
        }
    }
}
