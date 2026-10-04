package com.eroideches.omnidrop

import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.provider.Settings
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.Executors

/**
 * Hosts the Flutter UI and exposes the Android-only features the Rust engine cannot reach from
 * native code: BLE advertising/scanning, Wi-Fi Direct, the foreground transfer service, the
 * multicast lock needed by mDNS, notifications, file opening and "Share to OmniDrop".
 */
class MainActivity : FlutterActivity() {
    private val main = Handler(Looper.getMainLooper())
    private val io = Executors.newSingleThreadExecutor()
    private var sink: EventChannel.EventSink? = null
    private val pending = mutableListOf<Map<String, Any?>>()
    private var multicastLock: WifiManager.MulticastLock? = null
    private lateinit var offGrid: OffGridManager

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val messenger = flutterEngine.dartExecutor.binaryMessenger
        offGrid = OffGridManager(applicationContext) { event -> main.post { emit(event) } }
        EventChannel(messenger, "omnidrop/platform/events").setStreamHandler(
            object : EventChannel.StreamHandler {
                override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
                    sink = events
                    pending.forEach { events.success(it) }
                    pending.clear()
                }

                override fun onCancel(arguments: Any?) {
                    sink = null
                }
            },
        )
        MethodChannel(messenger, "omnidrop/platform").setMethodCallHandler { call, result ->
            try {
                handle(call, result)
            } catch (e: SecurityException) {
                result.error("permission", e.message, null)
            } catch (e: Exception) {
                result.error("error", e.message, null)
            }
        }
        handleShare(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleShare(intent)
    }

    override fun onDestroy() {
        offGrid.stopBle()
        offGrid.stopP2p()
        multicastLock?.takeIf { it.isHeld }?.release()
        io.shutdown()
        super.onDestroy()
    }

    private fun emit(event: Map<String, Any?>) {
        val s = sink
        if (s != null) {
            s.success(event)
        } else if (event["type"] == "shared") {
            pending.add(event)
        }
    }

    private fun handle(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "deviceName" -> result.success(deviceName())
            "downloadDir" -> {
                val dir = File(
                    Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS),
                    "OmniDrop",
                )
                dir.mkdirs()
                result.success(dir.absolutePath)
            }
            "appDownloadDir" -> {
                val base = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: filesDir
                val dir = File(base, "OmniDrop")
                dir.mkdirs()
                result.success(dir.absolutePath)
            }
            "storageRoot" -> result.success(Environment.getExternalStorageDirectory().absolutePath)
            "multicastLock" -> {
                setMulticastLock(call.argument<Boolean>("enable") == true)
                result.success(null)
            }
            "transferService" -> {
                TransferService.update(
                    this,
                    call.argument<String>("title") ?: "OmniDrop",
                    call.argument<String>("text") ?: "",
                    call.argument<Int>("progress") ?: -1,
                )
                result.success(null)
            }
            "stopTransferService" -> {
                TransferService.stop(this)
                result.success(null)
            }
            "incomingNotification" -> {
                Notifications.showIncoming(
                    this,
                    call.argument<String>("title") ?: "OmniDrop",
                    call.argument<String>("text") ?: "",
                )
                result.success(null)
            }
            "cancelIncomingNotification" -> {
                Notifications.cancelIncoming(this)
                result.success(null)
            }
            "bleStart" -> {
                val payload = call.argument<ByteArray>("payload")
                val company = call.argument<Int>("companyId") ?: 0xFFFF
                result.success(payload != null && offGrid.startBle(payload, company))
            }
            "bleStop" -> {
                offGrid.stopBle()
                result.success(null)
            }
            "p2pStart" -> result.success(
                offGrid.startP2p(
                    call.argument<String>("id") ?: "",
                    call.argument<String>("name") ?: "",
                    call.argument<String>("os") ?: "android",
                    call.argument<Int>("port") ?: 44850,
                ),
            )
            "p2pDiscover" -> {
                offGrid.discover()
                result.success(null)
            }
            "p2pConnect" -> result.success(offGrid.connect(call.argument<String>("address") ?: ""))
            "p2pDisconnect" -> {
                offGrid.disconnect()
                result.success(null)
            }
            "p2pStop" -> {
                offGrid.stopP2p()
                result.success(null)
            }
            "openFile" -> result.success(openFile(call.argument<String>("path") ?: ""))
            "openFolder" -> result.success(openFolder(call.argument<String>("path") ?: ""))
            "openUrl" -> {
                val url = call.argument<String>("url") ?: ""
                result.success(startSafely(Intent(Intent.ACTION_VIEW, Uri.parse(url))))
            }
            else -> result.notImplemented()
        }
    }

    private fun deviceName(): String {
        val global = Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME)
        if (!global.isNullOrBlank()) return global
        val model = Build.MODEL ?: "Android"
        val maker = Build.MANUFACTURER ?: ""
        return if (model.startsWith(maker, ignoreCase = true)) model else "$maker $model".trim()
    }

    private fun setMulticastLock(enable: Boolean) {
        if (enable) {
            if (multicastLock == null) {
                val wifi = applicationContext.getSystemService(WifiManager::class.java)
                multicastLock = wifi?.createMulticastLock("omnidrop-mdns")?.apply { setReferenceCounted(false) }
            }
            multicastLock?.takeIf { !it.isHeld }?.acquire()
        } else {
            multicastLock?.takeIf { it.isHeld }?.release()
        }
    }

    private fun startSafely(intent: Intent): Boolean = try {
        startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        true
    } catch (e: ActivityNotFoundException) {
        false
    }

    private fun mimeOf(file: File): String {
        val ext = file.extension.lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
    }

    private fun openFile(path: String): Boolean {
        val file = File(path)
        if (!file.exists()) return false
        val uri = FileProvider.getUriForFile(this, "$packageName.files", file)
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, mimeOf(file))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        return startSafely(Intent.createChooser(intent, file.name))
    }

    private fun openFolder(path: String): Boolean {
        val root = Environment.getExternalStorageDirectory().absolutePath
        val target = File(path).let { if (it.isDirectory) it else it.parentFile ?: it }
        if (target.absolutePath.startsWith(root)) {
            val rel = target.absolutePath.removePrefix(root).trimStart('/')
            val uri = DocumentsContract.buildDocumentUri(
                "com.android.externalstorage.documents",
                "primary:$rel",
            )
            val intent = Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, DocumentsContract.Document.MIME_TYPE_DIR)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            if (startSafely(intent)) return true
        }
        return startSafely(Intent(DownloadManager.ACTION_VIEW_DOWNLOADS))
    }

    // ------------------------------------------------------------------ Share to OmniDrop

    @Suppress("DEPRECATION")
    private fun handleShare(intent: Intent?) {
        if (intent == null) return
        val action = intent.action
        if (action != Intent.ACTION_SEND && action != Intent.ACTION_SEND_MULTIPLE) return
        val uris = mutableListOf<Uri>()
        if (action == Intent.ACTION_SEND) {
            val uri = if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
            }
            if (uri != null) uris.add(uri)
        } else {
            val list = if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
            }
            if (list != null) uris.addAll(list)
        }
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)
        // Consume the intent so a configuration change does not share twice.
        setIntent(Intent(this, MainActivity::class.java))
        io.execute {
            val paths = uris.mapNotNull { materialize(it) }
            main.post {
                emit(mapOf("type" to "shared", "paths" to paths, "text" to if (uris.isEmpty()) text else null))
            }
        }
    }

    /** Returns a real path for a shared URI, copying content:// streams into the cache. */
    private fun materialize(uri: Uri): String? {
        if (uri.scheme == "file") return uri.path
        return try {
            val dir = File(cacheDir, "shared").apply { mkdirs() }
            var name = "shared"
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) name = c.getString(0) ?: name
            }
            name = name.replace('/', '_')
            var dest = File(dir, name)
            var i = 1
            while (dest.exists()) {
                dest = File(dir, "${dest.nameWithoutExtension.substringBefore(" (")} (${i++})" +
                    (if (dest.extension.isNotEmpty()) ".${dest.extension}" else ""))
            }
            contentResolver.openInputStream(uri)?.use { input ->
                FileOutputStream(dest).use { output -> input.copyTo(output, 1 shl 20) }
            } ?: return null
            dest.absolutePath
        } catch (e: Exception) {
            null
        }
    }
}
