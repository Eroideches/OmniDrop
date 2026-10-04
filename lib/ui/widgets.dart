import 'package:flutter/material.dart';
import 'package:font_awesome_flutter/font_awesome_flutter.dart';

import '../core/format.dart';
import '../core/models.dart';

IconData osIcon(OsKind os) {
  switch (os) {
    case OsKind.android:
      return FontAwesomeIcons.android.data;
    case OsKind.windows:
      return FontAwesomeIcons.windows.data;
    case OsKind.linux:
      return FontAwesomeIcons.linux.data;
    case OsKind.unknown:
      return FontAwesomeIcons.desktop.data;
  }
}

Color osColor(OsKind os) {
  switch (os) {
    case OsKind.android:
      return const Color(0xFF3DDC84);
    case OsKind.windows:
      return const Color(0xFF3AA0FF);
    case OsKind.linux:
      return const Color(0xFFFFC53D);
    case OsKind.unknown:
      return const Color(0xFFB0BEC5);
  }
}

class OsAvatar extends StatelessWidget {
  const OsAvatar({super.key, required this.os, this.size = 44, this.glow = false});

  final OsKind os;
  final double size;
  final bool glow;

  @override
  Widget build(BuildContext context) {
    final color = osColor(os);
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        gradient: RadialGradient(colors: [color.withValues(alpha: 0.35), color.withValues(alpha: 0.12)]),
        border: Border.all(color: color.withValues(alpha: 0.9), width: 1.6),
        boxShadow: glow ? [BoxShadow(color: color.withValues(alpha: 0.55), blurRadius: 18, spreadRadius: 1)] : null,
      ),
      alignment: Alignment.center,
      child: Icon(osIcon(os), size: size * 0.46, color: color),
    );
  }
}

IconData fileKindIcon(FileKind kind) {
  switch (kind) {
    case FileKind.image:
      return Icons.image_outlined;
    case FileKind.video:
      return Icons.movie_outlined;
    case FileKind.audio:
      return Icons.music_note_outlined;
    case FileKind.archive:
      return Icons.folder_zip_outlined;
    case FileKind.document:
      return Icons.description_outlined;
    case FileKind.apk:
      return Icons.android;
    case FileKind.folder:
      return Icons.folder_outlined;
    case FileKind.other:
      return Icons.insert_drive_file_outlined;
  }
}

IconData transportIcon(String t) {
  switch (t) {
    case 'ble':
      return Icons.bluetooth;
    case 'wifi_direct':
      return Icons.wifi_tethering;
    case 'mdns':
    case 'broadcast':
      return Icons.wifi;
    default:
      return Icons.lan_outlined;
  }
}

String transportLabel(String t) {
  switch (t) {
    case 'ble':
      return 'Bluetooth LE';
    case 'wifi_direct':
      return 'Wi-Fi Direct';
    case 'mdns':
      return 'mDNS';
    case 'broadcast':
      return 'UDP broadcast';
    case 'manual':
      return 'IP';
    case 'incoming':
      return 'LAN';
    default:
      return t;
  }
}

/// The Short Authentication String: 6-digit PIN and 4 emoji.
class SasView extends StatelessWidget {
  const SasView({super.key, required this.sas, this.compact = false});

  final Sas sas;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final pinStyle = (compact ? theme.textTheme.titleLarge : theme.textTheme.displaySmall)?.copyWith(
      fontWeight: FontWeight.w700,
      letterSpacing: compact ? 3 : 6,
      fontFeatures: const [FontFeature.tabularFigures()],
      color: theme.colorScheme.primary,
    );
    return Container(
      padding: EdgeInsets.symmetric(horizontal: compact ? 12 : 18, vertical: compact ? 8 : 14),
      decoration: BoxDecoration(
        color: theme.colorScheme.primary.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: theme.colorScheme.primary.withValues(alpha: 0.35)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(sas.pinGrouped, style: pinStyle),
          SizedBox(height: compact ? 2 : 6),
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              for (final e in sas.emoji)
                Padding(
                  padding: EdgeInsets.symmetric(horizontal: compact ? 3 : 6),
                  child: Text(e, style: TextStyle(fontSize: compact ? 20 : 30)),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

class EmptyState extends StatelessWidget {
  const EmptyState({super.key, required this.icon, required this.title, this.subtitle});

  final IconData icon;
  final String title;
  final String? subtitle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 56, color: theme.colorScheme.outline),
            const SizedBox(height: 16),
            Text(title, style: theme.textTheme.titleMedium, textAlign: TextAlign.center),
            if (subtitle != null) ...[
              const SizedBox(height: 8),
              Text(
                subtitle!,
                style: theme.textTheme.bodyMedium?.copyWith(color: theme.colorScheme.onSurfaceVariant),
                textAlign: TextAlign.center,
              ),
            ],
          ],
        ),
      ),
    );
  }
}
