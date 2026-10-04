package com.eroideches.omnidrop

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.wifi.WpsInfo
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pDevice
import android.net.wifi.p2p.WifiP2pManager
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceInfo
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceRequest
import android.os.Build
import android.os.Looper
import android.os.SystemClock

/**
 * Off-grid discovery and links on Android.
 *
 * BLE: advertises the beacon built by the Rust engine as manufacturer-specific data and scans
 * (hardware-filtered on the "OD" prefix) for other OmniDrop devices; raw payloads are handed back
 * to the engine, which owns the beacon format.
 *
 * Wi-Fi Direct: publishes a DNS-SD service ("_omnidrop._tcp") carrying the OmniDrop device id, so
 * a peer seen over BLE can be mapped to its Wi-Fi Direct address and connected. The connecting
 * side uses group-owner intent 0, becomes the P2P client and probes the group owner (192.168.49.1),
 * after which the normal encrypted transfer runs over the P2P interface.
 */
class OffGridManager(
    private val context: Context,
    private val emit: (Map<String, Any?>) -> Unit,
) {
    private val bluetooth = context.getSystemService(BluetoothManager::class.java)
    private var advertiser: BluetoothLeAdvertiser? = null
    private var scanner: BluetoothLeScanner? = null
    private var advertiseCallback: AdvertiseCallback? = null
    private var scanCallback: ScanCallback? = null
    private val lastSeen = HashMap<String, Long>()

    private val p2p: WifiP2pManager? =
        if (context.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI_DIRECT)) {
            context.getSystemService(WifiP2pManager::class.java)
        } else {
            null
        }
    private var channel: WifiP2pManager.Channel? = null
    private var receiver: BroadcastReceiver? = null
    private var peers: List<WifiP2pDevice> = emptyList()
    private val omnidropIds = HashMap<String, String>()

    private fun granted(permission: String) =
        context.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED

    private fun hasBlePermissions(): Boolean =
        if (Build.VERSION.SDK_INT >= 31) {
            granted(Manifest.permission.BLUETOOTH_SCAN) &&
                granted(Manifest.permission.BLUETOOTH_ADVERTISE) &&
                granted(Manifest.permission.BLUETOOTH_CONNECT)
        } else {
            granted(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    private fun hasP2pPermissions(): Boolean =
        if (Build.VERSION.SDK_INT >= 33) {
            granted(Manifest.permission.NEARBY_WIFI_DEVICES)
        } else {
            granted(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    private fun bleState(active: Boolean, error: String? = null) =
        emit(mapOf("type" to "bleState", "active" to active, "error" to error))

    // ------------------------------------------------------------------ BLE

    @SuppressLint("MissingPermission")
    fun startBle(payload: ByteArray, companyId: Int): Boolean {
        if (!hasBlePermissions()) {
            bleState(false, "Bluetooth permission not granted")
            return false
        }
        val adapter = bluetooth?.adapter
        if (adapter == null) {
            bleState(false, "No Bluetooth adapter")
            return false
        }
        if (!adapter.isEnabled) {
            bleState(false, "Bluetooth is off")
            return false
        }
        stopBle()

        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
            .setConnectable(false)
            .setTimeout(0)
            .build()
        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .setIncludeTxPowerLevel(false)
            .addManufacturerData(companyId, payload)
            .build()
        val adv = object : AdvertiseCallback() {
            override fun onStartFailure(errorCode: Int) {
                bleState(scanCallback != null, "BLE advertising failed ($errorCode)")
            }
        }
        advertiseCallback = adv
        advertiser = adapter.bluetoothLeAdvertiser
        try {
            advertiser?.startAdvertising(settings, data, adv)
        } catch (e: Exception) {
            bleState(false, e.message)
        }

        val filter = ScanFilter.Builder()
            .setManufacturerData(
                companyId,
                byteArrayOf('O'.code.toByte(), 'D'.code.toByte()),
                byteArrayOf(0xFF.toByte(), 0xFF.toByte()),
            )
            .build()
        val scanSettings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_BALANCED)
            .build()
        val scan = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) {
                val bytes = result.scanRecord?.getManufacturerSpecificData(companyId) ?: return
                val key = result.device?.address ?: return
                val now = SystemClock.elapsedRealtime()
                if (now - (lastSeen[key] ?: 0L) < 1500) return
                lastSeen[key] = now
                emit(mapOf("type" to "ble", "data" to bytes, "rssi" to result.rssi))
            }

            override fun onBatchScanResults(results: MutableList<ScanResult>) {
                results.forEach { onScanResult(ScanSettings.CALLBACK_TYPE_ALL_MATCHES, it) }
            }

            override fun onScanFailed(errorCode: Int) {
                bleState(false, "BLE scan failed ($errorCode)")
            }
        }
        scanCallback = scan
        scanner = adapter.bluetoothLeScanner
        try {
            scanner?.startScan(listOf(filter), scanSettings, scan)
        } catch (e: Exception) {
            bleState(false, e.message)
            return false
        }
        bleState(true)
        return true
    }

    @SuppressLint("MissingPermission")
    fun stopBle() {
        try {
            advertiseCallback?.let { advertiser?.stopAdvertising(it) }
            scanCallback?.let { scanner?.stopScan(it) }
        } catch (e: Exception) {
            // Adapter turned off or permission revoked: nothing left to stop.
        }
        advertiseCallback = null
        scanCallback = null
    }

    // ------------------------------------------------------------------ Wi-Fi Direct

    private fun listener(what: String) = object : WifiP2pManager.ActionListener {
        override fun onSuccess() {}

        override fun onFailure(reason: Int) {
            val text = when (reason) {
                WifiP2pManager.P2P_UNSUPPORTED -> "Wi-Fi Direct is not supported"
                WifiP2pManager.BUSY -> "Wi-Fi Direct is busy"
                else -> "Wi-Fi Direct $what failed ($reason)"
            }
            emit(mapOf("type" to "p2pError", "message" to text))
        }
    }

    private fun statusName(status: Int) = when (status) {
        WifiP2pDevice.CONNECTED -> "connected"
        WifiP2pDevice.INVITED -> "invited"
        WifiP2pDevice.FAILED -> "failed"
        WifiP2pDevice.AVAILABLE -> "available"
        else -> "unavailable"
    }

    private fun emitPeers() {
        emit(
            mapOf(
                "type" to "p2pPeers",
                "peers" to peers.map {
                    mapOf(
                        "address" to it.deviceAddress,
                        "name" to it.deviceName,
                        "status" to statusName(it.status),
                        "omnidropId" to omnidropIds[it.deviceAddress],
                    )
                },
            ),
        )
    }

    @SuppressLint("MissingPermission")
    private fun registerReceiver(mgr: WifiP2pManager, ch: WifiP2pManager.Channel) {
        if (receiver != null) return
        val r = object : BroadcastReceiver() {
            override fun onReceive(ctx: Context, intent: Intent) {
                try {
                    when (intent.action) {
                        WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION -> mgr.requestPeers(ch) { list ->
                            peers = list?.deviceList?.toList() ?: emptyList()
                            emitPeers()
                        }
                        WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION -> mgr.requestConnectionInfo(ch) { info ->
                            if (info != null && info.groupFormed) {
                                emit(
                                    mapOf(
                                        "type" to "p2pConnected",
                                        "isGroupOwner" to info.isGroupOwner,
                                        "groupOwnerAddress" to info.groupOwnerAddress?.hostAddress,
                                    ),
                                )
                            } else {
                                emit(mapOf("type" to "p2pDisconnected"))
                            }
                        }
                        WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                            val state = intent.getIntExtra(WifiP2pManager.EXTRA_WIFI_STATE, -1)
                            if (state != WifiP2pManager.WIFI_P2P_STATE_ENABLED) {
                                emit(mapOf("type" to "p2pError", "message" to "Wi-Fi is off"))
                            }
                        }
                    }
                } catch (e: SecurityException) {
                    emit(mapOf("type" to "p2pError", "message" to "Wi-Fi Direct permission not granted"))
                }
            }
        }
        val filter = IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
        }
        if (Build.VERSION.SDK_INT >= 33) {
            context.registerReceiver(r, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            context.registerReceiver(r, filter)
        }
        receiver = r
    }

    @SuppressLint("MissingPermission")
    fun startP2p(id: String, name: String, os: String, port: Int): Boolean {
        val mgr = p2p
        if (mgr == null) {
            emit(mapOf("type" to "p2pError", "message" to "Wi-Fi Direct is not supported"))
            return false
        }
        if (!hasP2pPermissions()) {
            emit(mapOf("type" to "p2pError", "message" to "Wi-Fi Direct permission not granted"))
            return false
        }
        val ch = channel ?: mgr.initialize(context, Looper.getMainLooper()) { channel = null }
        channel = ch
        registerReceiver(mgr, ch)
        val record = mapOf("id" to id, "n" to name.take(32), "o" to os, "p" to port.toString())
        val service = WifiP2pDnsSdServiceInfo.newInstance("OmniDrop-$id", "_omnidrop._tcp", record)
        mgr.setDnsSdResponseListeners(
            ch,
            { _, _, _ -> },
            { _, txt, device ->
                val oid = txt["id"]
                if (oid != null && device != null) {
                    omnidropIds[device.deviceAddress] = oid
                    if (peers.none { it.deviceAddress == device.deviceAddress }) peers = peers + device
                    emitPeers()
                }
            },
        )
        mgr.clearLocalServices(
            ch,
            object : WifiP2pManager.ActionListener {
                override fun onSuccess() = mgr.addLocalService(ch, service, listener("service"))
                override fun onFailure(reason: Int) = mgr.addLocalService(ch, service, listener("service"))
            },
        )
        discover()
        return true
    }

    @SuppressLint("MissingPermission")
    fun discover() {
        val mgr = p2p ?: return
        val ch = channel ?: return
        if (!hasP2pPermissions()) return
        mgr.discoverPeers(ch, listener("discovery"))
        mgr.clearServiceRequests(
            ch,
            object : WifiP2pManager.ActionListener {
                override fun onSuccess() = request()
                override fun onFailure(reason: Int) = request()

                private fun request() {
                    mgr.addServiceRequest(ch, WifiP2pDnsSdServiceRequest.newInstance(), listener("service request"))
                    mgr.discoverServices(ch, listener("service discovery"))
                }
            },
        )
    }

    @SuppressLint("MissingPermission")
    @Suppress("DEPRECATION")
    fun connect(address: String): Boolean {
        val mgr = p2p ?: return false
        val ch = channel ?: return false
        if (address.isEmpty() || !hasP2pPermissions()) return false
        val config = WifiP2pConfig().apply {
            deviceAddress = address
            wps.setup = WpsInfo.PBC
            // Let the other device become group owner; we probe it as the client.
            groupOwnerIntent = 0
        }
        mgr.connect(ch, config, listener("connection"))
        return true
    }

    fun disconnect() {
        val mgr = p2p ?: return
        val ch = channel ?: return
        mgr.removeGroup(ch, listener("disconnect"))
    }

    fun stopP2p() {
        val mgr = p2p
        val ch = channel
        if (mgr != null && ch != null) {
            try {
                mgr.clearLocalServices(ch, null)
                mgr.clearServiceRequests(ch, null)
                mgr.stopPeerDiscovery(ch, null)
            } catch (e: Exception) {
                // Framework already tore the channel down.
            }
        }
        receiver?.let {
            try {
                context.unregisterReceiver(it)
            } catch (e: IllegalArgumentException) {
                // Not registered.
            }
        }
        receiver = null
    }
}
