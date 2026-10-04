import 'package:flutter_test/flutter_test.dart';
import 'package:omnidrop/core/format.dart';
import 'package:omnidrop/core/models.dart';

void main() {
  group('format', () {
    test('bytes', () {
      expect(formatBytes(999), '999 B');
      expect(formatBytes(1500), '1.5 kB');
      expect(formatBytes(250 * 1000 * 1000), '250 MB');
      expect(formatBytes(1.25e9), '1.3 GB');
    });

    test('speed and duration', () {
      expect(formatSpeed(112e6), '112 MB/s');
      expect(formatSpeed(2.5e6), '2.5 MB/s');
      expect(formatDuration(42), '42s');
      expect(formatDuration(125), '2m 05s');
      expect(formatDuration(3720), '1h 02m');
    });

    test('paths', () {
      expect(basename(r'C:\Users\me\photo.JPG'), 'photo.JPG');
      expect(basename('/home/me/dir/'), 'dir');
      expect(extensionOf('archive.tar.gz'), 'gz');
      expect(kindOf('movie.mkv'), FileKind.video);
      expect(kindOf('x', dir: true), FileKind.folder);
      expect(looksLikeUrl('https://example.org/a?b=1'), isTrue);
      expect(looksLikeUrl('hello world'), isFalse);
    });

    test('radar angle is stable', () {
      expect(angleFor('0011223344556677'), angleFor('0011223344556677'));
      expect(angleFor('a'), isNot(angleFor('b')));
    });
  });

  group('models', () {
    test('transfer snapshot', () {
      final t = TransferInfo.fromJson({
        'id': 'abc',
        'direction': 'receive',
        'peer': {'id': 'p', 'name': 'Pixel', 'os': 'android'},
        'state': 'transferring',
        'sas': {
          'pin': '123456',
          'emoji': ['🐶', '🦊', '🌵', '⚡'],
        },
        'total_bytes': 200,
        'done_bytes': 50,
        'speed': 1e6,
        'eta': 3,
        'files': [
          {'path': 'a.txt', 'size': 200, 'done': false, 'failed': false},
        ],
        'block_map': '210',
        'blocks_total': 3,
        'blocks_done': 1,
      });
      expect(t.outgoing, isFalse);
      expect(t.peer.os, OsKind.android);
      expect(t.progress, 0.25);
      expect(t.active, isTrue);
      expect(t.canPause, isTrue);
      expect(t.sas!.pinGrouped, '123 456');
    });

    test('device', () {
      final d = Device.fromJson({
        'id': '0011223344556677',
        'name': 'Laptop',
        'os': 'linux',
        'addresses': <String>[],
        'transports': ['ble'],
        'rssi': -60,
        'reachable': false,
        'trusted': true,
      });
      expect(d.bleOnly, isTrue);
      expect(d.os, OsKind.linux);
    });
  });
}
