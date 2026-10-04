package com.eroideches.omnidrop

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build

object Notifications {
    const val PROGRESS_ID = 4410
    private const val INCOMING_ID = 4411
    private const val CHANNEL_PROGRESS = "transfers"
    private const val CHANNEL_INCOMING = "incoming"

    private fun manager(context: Context) = context.getSystemService(NotificationManager::class.java)

    private fun ensureChannels(context: Context) {
        val m = manager(context) ?: return
        if (m.getNotificationChannel(CHANNEL_PROGRESS) == null) {
            m.createNotificationChannel(
                NotificationChannel(CHANNEL_PROGRESS, "Transfers", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Progress of running transfers"
                    setShowBadge(false)
                },
            )
        }
        if (m.getNotificationChannel(CHANNEL_INCOMING) == null) {
            m.createNotificationChannel(
                NotificationChannel(CHANNEL_INCOMING, "Incoming requests", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "Devices that want to send you files"
                },
            )
        }
    }

    private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private fun canPost(context: Context) = Build.VERSION.SDK_INT < 33 ||
        context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun progress(context: Context, title: String, text: String, progress: Int): Notification {
        ensureChannels(context)
        return Notification.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(R.drawable.ic_stat_omnidrop)
            .setContentTitle(title)
            .setContentText(text)
            .setOnlyAlertOnce(true)
            .setOngoing(true)
            .setContentIntent(openApp(context))
            .setCategory(Notification.CATEGORY_PROGRESS)
            .setProgress(100, progress.coerceIn(0, 100), progress < 0)
            .build()
    }

    fun notifyProgress(context: Context, title: String, text: String, progress: Int) {
        if (!canPost(context)) return
        manager(context)?.notify(PROGRESS_ID, progress(context, title, text, progress))
    }

    fun showIncoming(context: Context, title: String, text: String) {
        ensureChannels(context)
        if (!canPost(context)) return
        val n = Notification.Builder(context, CHANNEL_INCOMING)
            .setSmallIcon(R.drawable.ic_stat_omnidrop)
            .setContentTitle(title)
            .setContentText(text)
            .setAutoCancel(true)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setContentIntent(openApp(context))
            .build()
        manager(context)?.notify(INCOMING_ID, n)
    }

    fun cancelIncoming(context: Context) {
        manager(context)?.cancel(INCOMING_ID)
    }
}
