package io.github.morangsu.mneme

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/** 到期卡片通知：一个渠道 + 一个简单 Builder；小图标用系统资源，避免缺少 mipmap。 */
object Notify {

    private const val CHANNEL_ID = "mneme_due"
    private const val NOTIFICATION_ID = 1001

    fun notifyDue(context: Context, due: Int) {
        // Android 13+ 未授权时静默跳过，不抛异常。
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS)
            != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }

        val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "到期复习",
                NotificationManager.IMPORTANCE_DEFAULT,
            ).apply { description = "提醒有待复习的卡片" }
            manager.createNotificationChannel(channel)
        }

        val notification = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle("mneme · 待复习")
            .setContentText("有 $due 张卡片到期，打开 APP 开始复习")
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            .build()

        manager.notify(NOTIFICATION_ID, notification)
    }
}
