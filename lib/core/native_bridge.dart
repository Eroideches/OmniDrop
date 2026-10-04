import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

typedef _JsonFnC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _PollC = Pointer<Utf8> Function(Uint32);
typedef _PollDart = Pointer<Utf8> Function(int);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);
typedef _StopC = Void Function();
typedef _StopDart = void Function();

/// Loads the Rust core (`native/`) shipped next to the application.
DynamicLibrary openCoreLibrary() {
  if (Platform.isAndroid) return DynamicLibrary.open('libomnidrop_core.so');
  if (Platform.isWindows) return DynamicLibrary.open('omnidrop_core.dll');
  final exeDir = File(Platform.resolvedExecutable).parent.path;
  final bundled = '$exeDir/lib/libomnidrop_core.so';
  if (File(bundled).existsSync()) return DynamicLibrary.open(bundled);
  return DynamicLibrary.open('libomnidrop_core.so');
}

/// Thin JSON-over-FFI wrapper around the Rust engine.
class NativeCore {
  NativeCore._(DynamicLibrary lib)
    : _start = lib.lookupFunction<_JsonFnC, _JsonFnC>('omnidrop_start'),
      _command = lib.lookupFunction<_JsonFnC, _JsonFnC>('omnidrop_command'),
      _free = lib.lookupFunction<_FreeC, _FreeDart>('omnidrop_free_string'),
      _stop = lib.lookupFunction<_StopC, _StopDart>('omnidrop_stop');

  factory NativeCore.open() => NativeCore._(openCoreLibrary());

  final _JsonFnC _start;
  final _JsonFnC _command;
  final _FreeDart _free;
  final _StopDart _stop;
  final _events = StreamController<Map<String, dynamic>>.broadcast();
  Isolate? _poller;
  ReceivePort? _port;

  Stream<Map<String, dynamic>> get events => _events.stream;

  Map<String, dynamic> _call(_JsonFnC fn, Map<String, dynamic> arg) {
    final input = jsonEncode(arg).toNativeUtf8();
    try {
      final out = fn(input);
      if (out == nullptr) return {'ok': false, 'error': 'null result'};
      final text = out.toDartString();
      _free(out);
      return (jsonDecode(text) as Map).cast<String, dynamic>();
    } finally {
      malloc.free(input);
    }
  }

  Map<String, dynamic> start(Map<String, dynamic> config) => _call(_start, config);

  Map<String, dynamic> command(String cmd, [Map<String, dynamic> args = const {}]) =>
      _call(_command, {'cmd': cmd, ...args});

  /// Starts the background isolate that blocks on `omnidrop_poll_event`.
  Future<void> startEventPump() async {
    if (_poller != null) return;
    final port = ReceivePort();
    _port = port;
    port.listen((message) {
      if (message is String) {
        try {
          _events.add((jsonDecode(message) as Map).cast<String, dynamic>());
        } catch (_) {
          // Malformed event: ignore.
        }
      }
    });
    _poller = await Isolate.spawn(_pollLoop, port.sendPort, debugName: 'omnidrop-events');
  }

  void stop() {
    _poller?.kill(priority: Isolate.immediate);
    _poller = null;
    _port?.close();
    _stop();
  }

  static void _pollLoop(SendPort port) {
    final lib = openCoreLibrary();
    final poll = lib.lookupFunction<_PollC, _PollDart>('omnidrop_poll_event');
    final free = lib.lookupFunction<_FreeC, _FreeDart>('omnidrop_free_string');
    while (true) {
      final ptr = poll(400);
      if (ptr == nullptr) continue;
      final text = ptr.toDartString();
      free(ptr);
      port.send(text);
    }
  }
}
