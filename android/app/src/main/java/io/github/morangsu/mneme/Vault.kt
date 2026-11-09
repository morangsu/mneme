package io.github.morangsu.mneme

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.annotation.WorkerThread
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * 凭据保险箱：Android Keystore 里的 AES-256-GCM 密钥 + 私有目录里的密文。
 *
 * 为什么不是 `EncryptedSharedPreferences`：Jetpack Security 已被官方标为废弃，
 * 而它底下做的正是这件事（Keystore 包一层）。自己写 60 行，依赖为零，行为可读。
 *
 * 安全边界说清楚：
 * - 密钥在 Keystore 里，**进程拿不到明文密钥**，只能请求加解密；换机/恢复出厂后旧密文解不开。
 * - 密文放在 `SharedPreferences("mneme_vault")`，属于 APP 私有目录；即使被 root 读走，
 *   没有那台设备的 Keystore 密钥也解不开（这是它相对「明文 shared_prefs」的全部意义）。
 * - `sync.json` 里只写 `${SECRET:名字}`，真值只在这个进程的内存里出现过。
 *
 * 失败策略：**绝不回退明文**。Keystore 不可用（密钥被系统作废、模拟器缺 provider）时
 * 明确报错让用户重填 token，而不是悄悄写一份明文到磁盘——那等于安全功能反向降级。
 */
object Vault {

    private const val SP = "mneme_vault"
    private const val KEY_ALIAS = "mneme.vault.v1"
    private const val PREFIX = "cred:"
    private const val SCHEME = "v1"

    private fun sp(ctx: Context) = ctx.getSharedPreferences(SP, Context.MODE_PRIVATE)

    /** 取（或首次生成）Keystore 里的 AES 密钥。 */
    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.secretKey?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                // 不要求每次解锁都验证指纹/密码：后台同步（WorkManager）在锁屏时也要能读。
                .setUserAuthenticationRequired(false)
                .build(),
        )
        return gen.generateKey()
    }

    /** 存入（覆盖）。返回是否成功；失败必定抛在调用侧被捕获，不静默吞。 */
    @WorkerThread
    fun put(ctx: Context, name: String, secret: String): Boolean {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val ct = cipher.doFinal(secret.toByteArray(Charsets.UTF_8))
        val payload = "$SCHEME:${b64(cipher.iv)}:${b64(ct)}"
        return sp(ctx).edit().putString(PREFIX + name, payload).commit()
    }

    /**
     * 读取。解不开（密钥作废 / 换机 / 数据损坏）时返回 null 并**顺手清掉坏密文**，
     * 否则每次启动都会重复报同一个错，用户也不知道该重填。
     */
    @WorkerThread
    fun get(ctx: Context, name: String): String? {
        val payload = sp(ctx).getString(PREFIX + name, null) ?: return null
        val parts = payload.split(':')
        if (parts.size != 3 || parts[0] != SCHEME) {
            sp(ctx).edit().remove(PREFIX + name).apply()
            return null
        }
        return try {
            val iv = Base64.decode(parts[1], Base64.NO_WRAP)
            val ct = Base64.decode(parts[2], Base64.NO_WRAP)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
            String(cipher.doFinal(ct), Charsets.UTF_8)
        } catch (e: Exception) {
            sp(ctx).edit().remove(PREFIX + name).apply()
            null
        }
    }

    fun remove(ctx: Context, name: String) {
        sp(ctx).edit().remove(PREFIX + name).apply()
    }

    fun has(ctx: Context, name: String): Boolean = sp(ctx).contains(PREFIX + name)

    /** 已保存的凭据名（不含值）。 */
    fun names(ctx: Context): List<String> =
        sp(ctx).all.keys
            .filter { it.startsWith(PREFIX) }
            .map { it.removePrefix(PREFIX) }
            .sorted()

    private fun b64(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)
}
