import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

/// Native host features: Android radios/services through a method channel, desktop shell
/// integration through the OS tools.
class PlatformHost {
  static const _channel = MethodChannel('omnidrop/platform');
  static const _eventsChannel = EventChannel('omnidrop/platform/events');

  Stream<Map<String, dynamic>>? _events;

  /// Events from the Android host (BLE beacons, Wi-Fi Direct state, shared files).
  Stream<Map<String, dynamic>> get events {
    if (!Platform.isAndroid) return const Stream.empty();
    return _events ??= _eventsChannel
        .receiveBroadcastStream()
        .where((e) => e is Map)
        .map((e) => (e as Map).cast<String, dynamic>());
  }

  Future<T?> _invoke<T>(String method, [Map<String, dynamic>? args]) async {
    if (!Platform.isAndroid) return null;
    try {
      return await _channel.invokeMethod<T>(method, args);
    } on PlatformException {
      return null;
    } on MissingPluginException {
      return null;
    }
  }

  Future<String?> deviceName() => _invoke<String>('deviceName');

  Future<String?> publicDownloadDir() => _invoke<String>('downloadDir');

  Future<String?> appDownloadDir() => _invoke<String>('appDownloadDir');

  Future<String?> externalStorageRoot() => _invoke<String>('storageRoot');

  Future<void> setMulticastLock(bool enabled) => _invoke<void>('multicastLock', {'enable': enabled});

  Future<void> updateTransferService({required String title, required String text, required int progress}) =>
      _invoke<void>('transferService', {'title': title, 'text': text, 'progress': progress});

  Future<void> stopTransferService() => _invoke<void>('stopTransferService');

  Future<void> showIncomingNotification(String title, String text) =>
      _invoke<void>('incomingNotification', {'title': title, 'text': text});

  Future<void> cancelIncomingNotification() => _invoke<void>('cancelIncomingNotification');

  Future<bool> bleStart(Uint8List payload, int companyId) async =>
      await _invoke<bool>('bleStart', {'payload': payload, 'companyId': companyId}) ?? false;

  Future<void> bleStop() => _invoke<void>('bleStop');

  Future<bool> p2pStart({required String id, required String name, required String os, required int port}) async =>
      await _invoke<bool>('p2pStart', {'id': id, 'name': name, 'os': os, 'port': port}) ?? false;

  Future<void> p2pDiscover() => _invoke<void>('p2pDiscover');

  Future<bool> p2pConnect(String address) async => await _invoke<bool>('p2pConnect', {'address': address}) ?? false;

  Future<void> p2pDisconnect() => _invoke<void>('p2pDisconnect');

  Future<void> p2pStop() => _invoke<void>('p2pStop');

  /// Opens a received file with the default application.
  Future<void> openFile(String path) async {
    if (Platform.isAndroid) {
      await _invoke<bool>('openFile', {'path': path});
    } else if (Platform.isWindows) {
      await Process.run('explorer.exe', [path]);
    } else {
      await Process.run('xdg-open', [path]);
    }
  }

  /// Shows a file in its folder (selecting it where the platform supports it).
  Future<void> revealInFolder(String path) async {
    if (Platform.isAndroid) {
      await _invoke<bool>('openFolder', {'path': path});
    } else if (Platform.isWindows) {
      await Process.run('explorer.exe', ['/select,', path]);
    } else {
      final dir = FileSystemEntity.isDirectorySync(path) ? path : File(path).parent.path;
      await Process.run('xdg-open', [dir]);
    }
  }

  Future<void> openUrl(String url) async {
    final uri = Uri.tryParse(url.trim());
    if (uri == null || !(uri.isScheme('http') || uri.isScheme('https'))) return;
    if (Platform.isAndroid) {
      await _invoke<bool>('openUrl', {'url': uri.toString()});
    } else if (Platform.isWindows) {
      await Process.run('rundll32', ['url.dll,FileProtocolHandler', uri.toString()]);
    } else {
      await Process.run('xdg-open', [uri.toString()]);
    }
  }
}
