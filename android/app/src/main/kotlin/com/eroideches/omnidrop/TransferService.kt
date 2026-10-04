package com.eroideches.omnidrop

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager

/**
 * Foreground service (type dataSync) that keeps the process, CPU and Wi-Fi radio awake while the
 * Rust engine moves data, and shows the aggregated progress notification.
 */
class TransferService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    companion object {
        @Volatile
        private var running = false

        fun update(context: Context, title: String, text: String, progress: Int) {
            if (running) {
                Notifications.notifyProgress(context, title, text, progress)
                return
            }
            val intent = Intent(context, TransferService::class.java)
                .putExtra("title", title)
                .putExtra("text", text)
                .putExtra("progress", progress)
            try {
                context.startForegroundService(intent)
            } catch (e: Exception) {
                // Background start not allowed (Android 12+): the engine keeps working while the
                // process is alive; the notification appears once the app is visible again.
            }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, TransferService::class.java))
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = Notifications.progress(
            this,
            intent?.getStringExtra("title") ?: "OmniDrop",
            intent?.getStringExtra("text") ?: "",
            intent?.getIntExtra("progress", -1) ?: -1,
        )
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(Notifications.PROGRESS_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(Notifications.PROGRESS_ID, notification)
        }
        running = true
        acquireLocks()
        return START_NOT_STICKY
    }

    @Suppress("DEPRECATION")
    private fun acquireLocks() {
        if (wakeLock == null) {
            val pm = getSystemService(PowerManager::class.java)
            wakeLock = pm?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "omnidrop:transfer")?.apply {
                setReferenceCounted(false)
                acquire(6 * 60 * 60 * 1000L)
            }
        }
        if (wifiLock == null) {
            val wifi = applicationContext.getSystemService(WifiManager::class.java)
            val mode = if (Build.VERSION.SDK_INT >= 29) {
                WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            } else {
                WifiManager.WIFI_MODE_FULL_HIGH_PERF
            }
            wifiLock = wifi?.createWifiLock(mode, "omnidrop:transfer")?.apply {
                setReferenceCounted(false)
                acquire()
            }
        }
    }

    override fun onDestroy() {
        running = false
        wakeLock?.takeIf { it.isHeld }?.release()
        wifiLock?.takeIf { it.isHeld }?.release()
        wakeLock = null
        wifiLock = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
}
