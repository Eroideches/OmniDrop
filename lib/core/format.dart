import 'dart:math' as math;

/// Human readable byte size (decimal units, like file managers on every platform).
String formatBytes(num bytes, {int decimals = 1}) {
  if (bytes < 1000) return '${bytes.toInt()} B';
  const units = ['kB', 'MB', 'GB', 'TB', 'PB'];
  var value = bytes.toDouble();
  var unit = -1;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  final digits = value >= 100 ? 0 : decimals;
  return '${value.toStringAsFixed(digits)} ${units[unit]}';
}

String formatSpeed(double bytesPerSecond) {
  final mb = bytesPerSecond / 1e6;
  if (mb >= 10) return '${mb.toStringAsFixed(0)} MB/s';
  if (mb >= 0.1) return '${mb.toStringAsFixed(1)} MB/s';
  return '${(bytesPerSecond / 1e3).toStringAsFixed(0)} kB/s';
}

String formatDuration(int seconds) {
  if (seconds < 60) return '${seconds}s';
  final m = seconds ~/ 60;
  final s = seconds % 60;
  if (m < 60) return '${m}m ${s.toString().padLeft(2, '0')}s';
  final h = m ~/ 60;
  return '${h}h ${(m % 60).toString().padLeft(2, '0')}m';
}

String basename(String path) {
  final normalized = path.replaceAll('\\', '/');
  final trimmed = normalized.endsWith('/') ? normalized.substring(0, normalized.length - 1) : normalized;
  final i = trimmed.lastIndexOf('/');
  return i < 0 ? trimmed : trimmed.substring(i + 1);
}

String extensionOf(String path) {
  final name = basename(path);
  final i = name.lastIndexOf('.');
  return i <= 0 ? '' : name.substring(i + 1).toLowerCase();
}

const _imageExt = {'jpg', 'jpeg', 'png', 'webp', 'gif', 'bmp', 'heic', 'avif'};
const _videoExt = {'mp4', 'mkv', 'mov', 'avi', 'webm', 'm4v', '3gp'};
const _audioExt = {'mp3', 'flac', 'wav', 'ogg', 'm4a', 'aac', 'opus'};
const _archiveExt = {'zip', 'rar', '7z', 'tar', 'gz', 'xz', 'zst', 'bz2'};
const _docExt = {'pdf', 'doc', 'docx', 'odt', 'txt', 'md', 'rtf', 'xls', 'xlsx', 'ods', 'ppt', 'pptx', 'odp', 'csv'};

enum FileKind { image, video, audio, archive, document, apk, folder, other }

FileKind kindOf(String path, {bool dir = false}) {
  if (dir) return FileKind.folder;
  final ext = extensionOf(path);
  if (_imageExt.contains(ext)) return FileKind.image;
  if (_videoExt.contains(ext)) return FileKind.video;
  if (_audioExt.contains(ext)) return FileKind.audio;
  if (_archiveExt.contains(ext)) return FileKind.archive;
  if (_docExt.contains(ext)) return FileKind.document;
  if (ext == 'apk') return FileKind.apk;
  return FileKind.other;
}

bool isThumbnailable(String path) {
  const ext = {'jpg', 'jpeg', 'png', 'webp', 'gif', 'bmp'};
  return ext.contains(extensionOf(path));
}

bool looksLikeUrl(String text) {
  final t = text.trim();
  return !t.contains(RegExp(r'\s')) && (t.startsWith('http://') || t.startsWith('https://'));
}

/// Stable angle (radians) for a device id so a device keeps its place on the radar.
double angleFor(String id) {
  var h = 0;
  for (final c in id.codeUnits) {
    h = (h * 31 + c) & 0x7fffffff;
  }
  return (h % 3600) / 3600 * 2 * math.pi;
}
