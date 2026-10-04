import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:path_provider/path_provider.dart';
import 'package:permission_handler/permission_handler.dart';

import 'format.dart';
import 'models.dart';
import 'native_bridge.dart';
import 'platform_host.dart';

class ReceivedText {
  ReceivedText(this.from, this.text);

  final PeerRef from;
  final String text;
}

/// Content shared to OmniDrop from another Android app.
class SharedContent {
  SharedContent(this.paths, this.text);

  final List<String> paths;
  final String? text;
}

/// Application state: wraps the Rust engine and the platform host.
class OmniController extends ChangeNotifier with WidgetsBindingObserver {
  final host = PlatformHost();
  NativeCore? _core;

  bool ready = false;
  String? startError;
  String deviceId = '';
  String deviceName = '';
  OsKind os = OsKind.unknown;
  int port = 0;
  String publicKey = '';
  bool aesHw = false;
  String ioBackend = '';
  bool wifiDirectSupported = false;
  List<String> localAddresses = const [];
  EngineSettings settings = EngineSettings(const {});

  List<Device> devices = const [];
  final Map<String, TransferInfo> transfers = {};
  final Map<String, List<double>> speedHistory = {};

  bool bleActive = false;
  String? bleError;
  List<WifiDirectPeer> p2pPeers = const [];
  bool p2pScanning = false;
  bool p2pConnected = false;
  String? p2pError;

  final _incoming = StreamController<IncomingRequest>.broadcast();
  final _incomingCancelled = StreamController<String>.broadcast();
  final _texts = StreamController<ReceivedText>.broadcast();
  final _notices = StreamController<String>.broadcast();
  final _shared = StreamController<SharedContent>.broadcast();

  Stream<IncomingRequest> get incoming => _incoming.stream;
  Stream<String> get incomingCancelled => _incomingCancelled.stream;
  Stream<ReceivedText> get texts => _texts.stream;
  Stream<String> get notices => _notices.stream;
  Stream<SharedContent> get shared => _shared.stream;

  AppLifecycleState _lifecycle = AppLifecycleState.resumed;
  Timer? _bleRefresh;
  Timer? _scanTimer;
  String _blePayload = '';
  DateTime _lastServiceUpdate = DateTime.fromMillisecondsSinceEpoch(0);
  bool _serviceRunning = false;
  final List<StreamSubscription<dynamic>> _subs = [];

  int get activeTransfers => transfers.values.where((t) => t.active).length;

  Future<void> init() async {
    WidgetsBinding.instance.addObserver(this);
    try {
      final support = await getApplicationSupportDirectory();
      final download = await _defaultDownloadDir(support.path);
      final name = Platform.isAndroid ? await host.deviceName() : null;
      final core = NativeCore.open();
      _core = core;
      _subs.add(core.events.listen(_onEvent));
      await core.startEventPump();
      final r = core.start({
        'data_dir': support.path,
        'download_dir': download,
        'device_name': ?name,
        'host_radio': Platform.isAndroid,
      });
      if (r['ok'] != true) {
        startError = '${r['error']}';
        notifyListeners();
        return;
      }
      _applyStatus(r);
      if (Platform.isAndroid) {
        _subs.add(host.events.listen(_onHostEvent));
        await _migrateAndroidDownloadDir();
        await startAndroidRadios();
      }
      final d = core.command('devices');
      if (d['ok'] == true) _setDevices(d['devices']);
      ready = true;
      notifyListeners();
    } catch (e) {
      startError = '$e';
      notifyListeners();
    }
  }

  Future<String> _defaultDownloadDir(String support) async {
    final sep = Platform.pathSeparator;
    if (Platform.isAndroid) {
      final granted = await Permission.manageExternalStorage.isGranted;
      final public = granted ? await host.publicDownloadDir() : null;
      return public ?? await host.appDownloadDir() ?? '$support${sep}Received';
    }
    final d = await getDownloadsDirectory();
    return d != null ? '${d.path}${sep}OmniDrop' : '$support${sep}Received';
  }

  /// After "All files access" is granted, move from the app-private folder to Download/OmniDrop.
  Future<void> _migrateAndroidDownloadDir() async {
    if (!Platform.isAndroid || !await Permission.manageExternalStorage.isGranted) return;
    final private = await host.appDownloadDir();
    final public = await host.publicDownloadDir();
    if (public != null && private != null && settings.downloadDir == private) {
      updateSettings({'download_dir': public});
    }
  }

  void _applyStatus(Map<String, dynamic> r) {
    deviceId = r['id'] as String? ?? '';
    deviceName = r['name'] as String? ?? '';
    os = parseOs(r['os']);
    port = (r['port'] as num?)?.toInt() ?? 0;
    publicKey = r['public_key'] as String? ?? '';
    aesHw = r['aes_hw'] == true;
    ioBackend = r['io_backend'] as String? ?? '';
    wifiDirectSupported = r['wifi_direct_supported'] == true;
    localAddresses = (r['addresses'] as List? ?? const [])
        .whereType<Map>()
        .map((a) => '${a['ip']} (${a['interface']})')
        .toList();
    if (r['settings'] is Map) {
      settings = EngineSettings((r['settings'] as Map).cast<String, dynamic>());
    }
  }

  Map<String, dynamic> _cmd(String cmd, [Map<String, dynamic> args = const {}]) {
    final core = _core;
    if (core == null) return {'ok': false, 'error': 'engine not started'};
    return core.command(cmd, args);
  }

  void refreshStatus() {
    final r = _cmd('status');
    if (r['ok'] == true) {
      _applyStatus(r);
      notifyListeners();
    }
  }

  // ------------------------------------------------------------------ events

  void _setDevices(Object? list) {
    devices = (list as List? ?? const [])
        .whereType<Map>()
        .map((d) => Device.fromJson(d.cast<String, dynamic>()))
        .toList();
  }

  void _onEvent(Map<String, dynamic> e) {
    switch (e['event']) {
      case 'devices':
        _setDevices(e['devices']);
        notifyListeners();
      case 'transfer':
        final t = TransferInfo.fromJson(e);
        transfers[t.id] = t;
        final history = speedHistory.putIfAbsent(t.id, () => <double>[]);
        if (t.state == 'transferring') {
          history.add(t.speed / 1e6);
          if (history.length > 150) history.removeAt(0);
        }
        _updateForegroundService();
        notifyListeners();
      case 'incoming':
        final req = IncomingRequest.fromJson(e);
        if (req.auto) {
          _notices.add('${req.peer.name}: ${req.fileCount} file');
          return;
        }
        _incoming.add(req);
        if (_lifecycle != AppLifecycleState.resumed) {
          host.showIncomingNotification(
            req.peer.name,
            req.fileCount > 0 ? '${req.fileCount} file · ${formatBytes(req.totalSize)}' : (req.text ?? ''),
          );
        }
      case 'incoming_cancelled':
        _incomingCancelled.add(e['request_id'] as String? ?? '');
        host.cancelIncomingNotification();
      case 'text_received':
        _texts.add(ReceivedText(PeerRef.fromJson(e['from']), e['text'] as String? ?? ''));
      case 'settings':
        if (e['settings'] is Map) {
          settings = EngineSettings((e['settings'] as Map).cast<String, dynamic>());
          deviceName = settings.deviceName;
          notifyListeners();
        }
      case 'ble_state':
        bleActive = e['active'] == true;
        bleError = e['error'] as String?;
        notifyListeners();
      case 'wifi_direct':
        p2pScanning = false;
        p2pError = e['error'] as String?;
        p2pPeers = (e['peers'] as List? ?? const [])
            .whereType<Map>()
            .map(
              (p) => WifiDirectPeer(
                id: p['id'] as String? ?? '',
                name: p['name'] as String? ?? '',
                address: p['address'] as String? ?? '',
                status: p['status'] as String? ?? '',
              ),
            )
            .toList();
        notifyListeners();
      case 'wifi_direct_link':
        p2pConnected = e['connected'] == true;
        if (e['error'] is String) {
          p2pError = e['error'] as String;
          _notices.add(p2pError!);
        }
        notifyListeners();
      case 'probe_result':
        if (e['ok'] != true) _notices.add('${e['address']}: ${e['error']}');
      case 'log':
        if (kDebugMode) debugPrint('[omnidrop:${e['level']}] ${e['message']}');
    }
  }

  void _onHostEvent(Map<String, dynamic> e) {
    switch (e['type']) {
      case 'ble':
        final data = e['data'];
        if (data is Uint8List) {
          _cmd('ble_seen', {'data': base64Encode(data), 'rssi': e['rssi'] ?? -100});
        }
      case 'bleState':
        bleActive = e['active'] == true;
        bleError = e['error'] as String?;
        notifyListeners();
      case 'p2pPeers':
        p2pScanning = false;
        p2pPeers = (e['peers'] as List? ?? const [])
            .whereType<Map>()
            .map(
              (p) => WifiDirectPeer(
                id: p['address'] as String? ?? '',
                name: p['name'] as String? ?? '',
                address: p['address'] as String? ?? '',
                status: p['status'] as String? ?? '',
                omnidropId: p['omnidropId'] as String?,
              ),
            )
            .toList();
        notifyListeners();
      case 'p2pConnected':
        p2pConnected = true;
        p2pError = null;
        final go = e['groupOwnerAddress'] as String?;
        if (e['isGroupOwner'] != true && go != null && go.isNotEmpty) {
          _cmd('probe', {'address': go, 'transport': 'wifi_direct'});
        }
        _cmd('refresh');
        notifyListeners();
      case 'p2pDisconnected':
        p2pConnected = false;
        notifyListeners();
      case 'p2pError':
        p2pScanning = false;
        p2pError = e['message'] as String?;
        notifyListeners();
      case 'shared':
        final paths = (e['paths'] as List? ?? const []).cast<String>();
        final text = e['text'] as String?;
        if (paths.isNotEmpty || (text != null && text.isNotEmpty)) {
          _shared.add(SharedContent(paths, text));
        }
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _lifecycle = state;
    if (state == AppLifecycleState.resumed) {
      host.cancelIncomingNotification();
      _cmd('refresh');
      refreshStatus();
    }
  }

  void _updateForegroundService() {
    if (!Platform.isAndroid) return;
    final running = transfers.values.where((t) => t.active && t.state != 'waiting_accept').toList();
    if (running.isEmpty) {
      if (_serviceRunning) {
        _serviceRunning = false;
        host.stopTransferService();
      }
      return;
    }
    final now = DateTime.now();
    if (_serviceRunning && now.difference(_lastServiceUpdate) < const Duration(seconds: 1)) return;
    _lastServiceUpdate = now;
    _serviceRunning = true;
    final total = running.fold<int>(0, (a, t) => a + t.totalBytes);
    final done = running.fold<int>(0, (a, t) => a + t.doneBytes);
    final speed = running.fold<double>(0, (a, t) => a + t.speed);
    final first = running.first;
    host.updateTransferService(
      title: running.length == 1 ? '${first.outgoing ? '↑' : '↓'} ${first.peer.name}' : 'OmniDrop · ${running.length}',
      text: '${formatBytes(done)} / ${formatBytes(total)} · ${formatSpeed(speed)}',
      progress: total == 0 ? -1 : (done * 100 ~/ total),
    );
  }

  // ------------------------------------------------------------------ android radios

  Future<void> startAndroidRadios() async {
    if (!Platform.isAndroid || _core == null) return;
    await host.setMulticastLock(true);
    if (settings.ble) {
      await _advertiseBle(force: true);
      _bleRefresh ??= Timer.periodic(const Duration(seconds: 30), (_) => _advertiseBle());
    } else {
      _bleRefresh?.cancel();
      _bleRefresh = null;
      await host.bleStop();
      bleActive = false;
    }
    if (settings.wifiDirect) {
      await host.p2pStart(id: deviceId, name: deviceName, os: 'android', port: port);
    } else {
      await host.p2pStop();
    }
    notifyListeners();
  }

  Future<void> _advertiseBle({bool force = false}) async {
    final r = _cmd('ble_payload');
    if (r['ok'] != true) return;
    final payload = r['payload'] as String;
    if (!force && payload == _blePayload) return;
    _blePayload = payload;
    await host.bleStart(base64Decode(payload), (r['company_id'] as num).toInt());
  }

  // ------------------------------------------------------------------ actions

  /// Starts sending; returns the transfer id or throws a [StateError] with the reason.
  Future<String> send(Device device, List<String> paths, {String? text}) async {
    final thumbs = await _thumbnails(paths);
    final r = _cmd('send', {'peer_id': device.id, 'paths': paths, 'text': ?text, 'thumbs': thumbs});
    if (r['ok'] != true) throw StateError('${r['error']}');
    return r['transfer_id'] as String;
  }

  Future<Map<String, String>> _thumbnails(List<String> paths) async {
    final out = <String, String>{};
    for (final path in paths) {
      if (out.length >= 4) break;
      if (!isThumbnailable(path)) continue;
      try {
        final file = File(path);
        if (!file.existsSync() || file.lengthSync() > 25 * 1000 * 1000) continue;
        final codec = await ui.instantiateImageCodec(await file.readAsBytes(), targetWidth: 160);
        final frame = await codec.getNextFrame();
        final data = await frame.image.toByteData(format: ui.ImageByteFormat.png);
        frame.image.dispose();
        codec.dispose();
        if (data != null && data.lengthInBytes < 150000) {
          out[path] = base64Encode(data.buffer.asUint8List());
        }
      } catch (_) {
        // Not a decodable image: no preview.
      }
    }
    return out;
  }

  bool respond(String requestId, {required bool accept, bool trust = false}) {
    host.cancelIncomingNotification();
    return _cmd('respond', {'request_id': requestId, 'accept': accept, 'trust': trust})['ok'] == true;
  }

  void pause(String id) => _cmd('pause', {'transfer_id': id});

  void resume(String id) => _cmd('resume', {'transfer_id': id});

  void cancel(String id) => _cmd('cancel', {'transfer_id': id});

  void clearFinished() {
    _cmd('clear_finished');
    transfers.removeWhere((_, t) => t.terminal);
    speedHistory.removeWhere((id, _) => !transfers.containsKey(id));
    notifyListeners();
  }

  void removeTransfer(String id) {
    final t = transfers[id];
    if (t == null || !t.terminal) return;
    transfers.remove(id);
    speedHistory.remove(id);
    notifyListeners();
  }

  String? updateSettings(Map<String, dynamic> patch) {
    final r = _cmd('update_settings', {'settings': patch});
    if (r['ok'] != true) return '${r['error']}';
    settings = EngineSettings((r['settings'] as Map).cast<String, dynamic>());
    deviceName = settings.deviceName;
    if (Platform.isAndroid &&
        (patch.containsKey('ble') || patch.containsKey('wifi_direct') || patch.containsKey('device_name'))) {
      startAndroidRadios();
    }
    notifyListeners();
    return null;
  }

  void forgetTrusted(String id) => _cmd('forget_trusted', {'id': id});

  void refresh() {
    _cmd('refresh');
    if (Platform.isAndroid && settings.wifiDirect) host.p2pDiscover();
  }

  void probe(String address) => _cmd('probe', {'address': address});

  void wifiDirectScan() {
    p2pScanning = true;
    p2pError = null;
    notifyListeners();
    if (Platform.isAndroid) {
      host.p2pDiscover();
      _scanTimer?.cancel();
      _scanTimer = Timer(const Duration(seconds: 12), () {
        p2pScanning = false;
        notifyListeners();
      });
    } else {
      _cmd('wifi_direct_scan');
    }
  }

  Future<void> wifiDirectConnect(WifiDirectPeer peer) async {
    p2pError = null;
    notifyListeners();
    if (Platform.isAndroid) {
      final ok = await host.p2pConnect(peer.address);
      if (!ok) {
        p2pError = 'Wi-Fi Direct';
        notifyListeners();
      }
    } else {
      _cmd('wifi_direct_connect', {'peer': peer.id});
    }
  }

  /// Off-grid link to a device seen only over BLE. Returns false when the user must pick the
  /// Wi-Fi Direct peer manually.
  Future<bool> connectOffGrid(Device device) async {
    if (!Platform.isAndroid) return false;
    final match = p2pPeers.where((p) => p.omnidropId == device.id).toList();
    if (match.isEmpty) return false;
    await wifiDirectConnect(match.first);
    return true;
  }

  void wifiDirectDisconnect() {
    if (Platform.isAndroid) {
      host.p2pDisconnect();
    } else {
      _cmd('wifi_direct_disconnect');
    }
    p2pConnected = false;
    notifyListeners();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    for (final s in _subs) {
      s.cancel();
    }
    _bleRefresh?.cancel();
    _scanTimer?.cancel();
    _core?.stop();
    super.dispose();
  }
}

/// Exposes the controller to the widget tree.
class OmniScope extends InheritedNotifier<OmniController> {
  const OmniScope({super.key, required OmniController controller, required super.child}) : super(notifier: controller);

  static OmniController of(BuildContext context) => context.dependOnInheritedWidgetOfExactType<OmniScope>()!.notifier!;

  static OmniController read(BuildContext context) => context.getInheritedWidgetOfExactType<OmniScope>()!.notifier!;
}
