enum OsKind { android, windows, linux, unknown }

OsKind parseOs(Object? s) {
  switch (s) {
    case 'android':
      return OsKind.android;
    case 'windows':
      return OsKind.windows;
    case 'linux':
      return OsKind.linux;
    default:
      return OsKind.unknown;
  }
}

int _int(Object? v) => v is num ? v.toInt() : 0;
double _double(Object? v) => v is num ? v.toDouble() : 0;

class Device {
  Device({
    required this.id,
    required this.name,
    required this.os,
    required this.addresses,
    required this.transports,
    required this.rssi,
    required this.reachable,
    required this.trusted,
  });

  factory Device.fromJson(Map<String, dynamic> j) => Device(
    id: j['id'] as String? ?? '',
    name: j['name'] as String? ?? '',
    os: parseOs(j['os']),
    addresses: (j['addresses'] as List? ?? const []).cast<String>(),
    transports: (j['transports'] as List? ?? const []).cast<String>(),
    rssi: j['rssi'] is num ? (j['rssi'] as num).toInt() : null,
    reachable: j['reachable'] == true,
    trusted: j['trusted'] == true,
  );

  final String id;
  final String name;
  final OsKind os;
  final List<String> addresses;
  final List<String> transports;
  final int? rssi;
  final bool reachable;
  final bool trusted;

  bool get bleOnly => !reachable && transports.contains('ble');
  bool get viaWifiDirect => transports.contains('wifi_direct');
}

class Sas {
  Sas(this.pin, this.emoji);

  static Sas? fromJson(Object? j) {
    if (j is! Map) return null;
    return Sas(j['pin'] as String? ?? '', (j['emoji'] as List? ?? const []).cast<String>());
  }

  final String pin;
  final List<String> emoji;

  String get pinGrouped => pin.length == 6 ? '${pin.substring(0, 3)} ${pin.substring(3)}' : pin;
}

class PeerRef {
  PeerRef(this.id, this.name, this.os);

  factory PeerRef.fromJson(Object? j) {
    final m = j is Map ? j : const {};
    return PeerRef(m['id'] as String? ?? '', m['name'] as String? ?? '', parseOs(m['os']));
  }

  final String id;
  final String name;
  final OsKind os;
}

class TransferFile {
  TransferFile(this.path, this.size, this.done, this.failed);

  final String path;
  final int size;
  final bool done;
  final bool failed;
}

class TransferInfo {
  TransferInfo({
    required this.id,
    required this.outgoing,
    required this.peer,
    required this.state,
    required this.error,
    required this.sas,
    required this.cipher,
    required this.totalBytes,
    required this.doneBytes,
    required this.speed,
    required this.eta,
    required this.files,
    required this.fileCount,
    required this.currentFile,
    required this.blockMap,
    required this.blocksTotal,
    required this.blocksDone,
    required this.streams,
    required this.paused,
    required this.localPaused,
    required this.savedPaths,
    required this.destDir,
    required this.text,
    required this.startedMs,
  });

  factory TransferInfo.fromJson(Map<String, dynamic> j) => TransferInfo(
    id: j['id'] as String? ?? '',
    outgoing: j['direction'] == 'send',
    peer: PeerRef.fromJson(j['peer']),
    state: j['state'] as String? ?? 'preparing',
    error: j['error'] as String?,
    sas: Sas.fromJson(j['sas']),
    cipher: j['cipher'] as String?,
    totalBytes: _int(j['total_bytes']),
    doneBytes: _int(j['done_bytes']),
    speed: _double(j['speed']),
    eta: j['eta'] is num ? (j['eta'] as num).toInt() : null,
    files: (j['files'] as List? ?? const [])
        .whereType<Map>()
        .map((f) => TransferFile(f['path'] as String? ?? '', _int(f['size']), f['done'] == true, f['failed'] == true))
        .toList(),
    fileCount: _int(j['file_count']),
    currentFile: j['current_file'] as String?,
    blockMap: j['block_map'] as String? ?? '',
    blocksTotal: _int(j['blocks_total']),
    blocksDone: _int(j['blocks_done']),
    streams: _int(j['streams']),
    paused: j['paused'] == true,
    localPaused: j['local_paused'] == true,
    savedPaths: (j['saved_paths'] as List? ?? const []).cast<String>(),
    destDir: j['dest_dir'] as String? ?? '',
    text: j['text'] as String?,
    startedMs: _int(j['started_ms']),
  );

  final String id;
  final bool outgoing;
  final PeerRef peer;
  final String state;
  final String? error;
  final Sas? sas;
  final String? cipher;
  final int totalBytes;
  final int doneBytes;
  final double speed;
  final int? eta;
  final List<TransferFile> files;
  final int fileCount;
  final String? currentFile;
  final String blockMap;
  final int blocksTotal;
  final int blocksDone;
  final int streams;
  final bool paused;
  final bool localPaused;
  final List<String> savedPaths;
  final String destDir;
  final String? text;
  final int startedMs;

  bool get terminal => const {'completed', 'rejected', 'cancelled', 'failed'}.contains(state);
  bool get active => !terminal && state != 'interrupted';
  bool get canPause => const {'transferring', 'verifying', 'finalizing'}.contains(state);
  double get progress => totalBytes == 0 ? (state == 'completed' ? 1 : 0) : (doneBytes / totalBytes).clamp(0.0, 1.0);
}

class IncomingItem {
  IncomingItem(this.path, this.size, this.dir);

  final String path;
  final int size;
  final bool dir;
}

class IncomingRequest {
  IncomingRequest({
    required this.requestId,
    required this.transferId,
    required this.peer,
    required this.sas,
    required this.cipher,
    required this.items,
    required this.itemCount,
    required this.fileCount,
    required this.totalSize,
    required this.text,
    required this.thumbs,
    required this.trusted,
    required this.auto,
  });

  factory IncomingRequest.fromJson(Map<String, dynamic> j) => IncomingRequest(
    requestId: j['request_id'] as String? ?? '',
    transferId: j['transfer_id'] as String? ?? '',
    peer: PeerRef.fromJson(j['peer']),
    sas: Sas.fromJson(j['sas']) ?? Sas('', const []),
    cipher: j['cipher'] as String? ?? '',
    items: (j['items'] as List? ?? const [])
        .whereType<Map>()
        .map((i) => IncomingItem(i['path'] as String? ?? '', _int(i['size']), i['dir'] == true))
        .toList(),
    itemCount: _int(j['item_count']),
    fileCount: _int(j['file_count']),
    totalSize: _int(j['total_size']),
    text: j['text'] as String?,
    thumbs: (j['thumbs'] as Map? ?? const {}).map((k, v) => MapEntry(int.tryParse('$k') ?? -1, v as String)),
    trusted: j['trusted'] == true,
    auto: j['auto'] == true,
  );

  final String requestId;
  final String transferId;
  final PeerRef peer;
  final Sas sas;
  final String cipher;
  final List<IncomingItem> items;
  final int itemCount;
  final int fileCount;
  final int totalSize;
  final String? text;
  final Map<int, String> thumbs;
  final bool trusted;
  final bool auto;
}

class TrustedPeer {
  TrustedPeer(this.id, this.name, this.os, this.publicKey);

  final String id;
  final String name;
  final OsKind os;
  final String publicKey;
}

class EngineSettings {
  EngineSettings(this.raw);

  final Map<String, dynamic> raw;

  String get deviceName => raw['device_name'] as String? ?? '';
  String get downloadDir => raw['download_dir'] as String? ?? '';
  bool get mdns => raw['mdns'] != false;
  bool get broadcast => raw['broadcast'] != false;
  bool get ble => raw['ble'] != false;
  bool get wifiDirect => raw['wifi_direct'] != false;
  int get streams => _int(raw['streams']).clamp(1, 8);
  bool get autoAcceptTrusted => raw['auto_accept_trusted'] == true;
  List<TrustedPeer> get trusted => (raw['trusted'] as List? ?? const [])
      .whereType<Map>()
      .map(
        (t) => TrustedPeer(
          t['id'] as String? ?? '',
          t['name'] as String? ?? '',
          parseOs(t['os']),
          t['public_key'] as String? ?? '',
        ),
      )
      .toList();
}

class WifiDirectPeer {
  WifiDirectPeer({required this.id, required this.name, required this.address, required this.status, this.omnidropId});

  final String id;
  final String name;
  final String address;
  final String status;
  final String? omnidropId;
}
