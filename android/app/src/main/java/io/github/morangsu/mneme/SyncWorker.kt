package io.github.morangsu.mneme

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters

/**
 * 后台周期同步：由 MainActivity 以 unique periodic work 入队，
 * 约束 NetworkType.CONNECTED，每小时一次；充电约束为可选（见 MainActivity 注释）。
 * 网络同步逻辑完全在 Rust 侧 `mneme-sync`，这里只调 FFI 的一个方法。
 */
class SyncWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {

    override suspend fun doWork(): Result = try {
        val receipt = Api.sync("github-private", false)
        if (receipt.has("error")) {
            Result.retry()
        } else {
            // 同步成功后顺便提醒到期卡片（无通知权限时 Notify 内部会直接返回）。
            val due = Api.quiz(1u).optInt("due", 0)
            if (due > 0) Notify.notifyDue(applicationContext, due)
            Result.success()
        }
    } catch (e: Exception) {
        Result.retry()
    }

    companion object {
        const val UNIQUE_NAME = "mneme-periodic-sync"
    }
}
