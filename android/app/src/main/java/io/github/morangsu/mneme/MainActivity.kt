package io.github.morangsu.mneme

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.lifecycle.lifecycleScope
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import kotlinx.coroutines.launch
import java.util.concurrent.TimeUnit

class MainActivity : ComponentActivity() {

    // 在 Activity 构造阶段注册（官方推荐写法），onCreate 里按需 launch。
    private val requestNotificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { /* 拒绝也不阻塞主流程 */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Android 13+ 通知需运行时授权；到期卡片提醒依赖它。
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
            != PackageManager.PERMISSION_GRANTED
        ) {
            requestNotificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }

        // 把数据目录与 deviceId 交给 Rust 侧（本地 SQLite 由 mneme-store 管理）。
        // 开库之后立刻把 Keystore 里的凭据注入 Rust 内存——Rust 侧的凭据注册表是进程级的，
        // 进程每次重启都会空，配置里只有 `${SECRET:…}` 引用，没注入就同步不了。
        lifecycleScope.launch {
            Db.init(this@MainActivity)
            Credentials.bootstrap(this@MainActivity)
        }

        // 后台周期同步：联网时每小时跑一次，唯一任务避免重复堆叠。
        enqueueSyncWorker()

        // 外观偏好先于组合恢复，避免首帧闪一下暗色。
        MnemeAppearance.mode = getSharedPreferences("mneme_ui", MODE_PRIVATE)
            .getString("theme", "system") ?: "system"

        setContent {
            MnemeTheme {
                MnemeApp()
            }
        }
    }

    private fun enqueueSyncWorker() {
        val constraints = Constraints.Builder()
            .setRequiredNetworkType(NetworkType.CONNECTED)
            // 「仅充电时同步」是可选项，默认不强制；需要时取消下一行注释：
            // .setRequiresCharging(true)
            .build()
        val request = PeriodicWorkRequestBuilder<SyncWorker>(1, TimeUnit.HOURS)
            .setConstraints(constraints)
            .build()
        WorkManager.getInstance(this).enqueueUniquePeriodicWork(
            SyncWorker.UNIQUE_NAME,
            ExistingPeriodicWorkPolicy.KEEP,
            request,
        )
    }
}
