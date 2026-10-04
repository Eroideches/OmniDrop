import 'dart:io';
import 'dart:math' as math;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';

import '../core/format.dart';
import '../core/models.dart';
import '../l10n.dart';
import 'widgets.dart';

bool get isDesktop => Platform.isWindows || Platform.isLinux;

/// Radial animated map of nearby devices, grouped around this device by link type.
class RadarView extends StatefulWidget {
  const RadarView({
    super.key,
    required this.devices,
    required this.selfOs,
    required this.selfName,
    required this.onTap,
    required this.onLongPress,
    required this.onDrop,
  });

  final List<Device> devices;
  final OsKind selfOs;
  final String selfName;
  final ValueChanged<Device> onTap;
  final ValueChanged<Device> onLongPress;

  /// Files dropped on the radar (desktop); [target] is the device under the cursor, if any.
  final void Function(List<String> paths, Device? target) onDrop;

  @override
  State<RadarView> createState() => _RadarViewState();
}

class _RadarViewState extends State<RadarView> with TickerProviderStateMixin {
  late final AnimationController _sweep = AnimationController(vsync: this, duration: const Duration(seconds: 4))
    ..repeat();
  late final AnimationController _pulse = AnimationController(vsync: this, duration: const Duration(milliseconds: 2600))
    ..repeat();
  bool _dragging = false;
  String? _hoverId;
  Map<String, Offset> _positions = const {};

  static const _nodeW = 112.0;
  static const _nodeH = 92.0;

  @override
  void dispose() {
    _sweep.dispose();
    _pulse.dispose();
    super.dispose();
  }

  Map<String, Offset> _layout(Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = math.max(60.0, math.min(size.width, size.height) / 2 - 46);
    final out = <String, Offset>{};
    for (final d in widget.devices) {
      final angle = angleFor(d.id);
      double r;
      if (d.reachable) {
        // LAN / Wi-Fi Direct peers sit on the inner ring.
        r = radius * (0.5 + (angle % 0.6) / 0.6 * 0.12);
      } else {
        final rssi = (d.rssi ?? -90).clamp(-100, -40);
        r = radius * (0.68 + (-rssi - 40) / 60 * 0.3);
      }
      out[d.id] = center + Offset(math.cos(angle) * r, math.sin(angle) * r);
    }
    return out;
  }

  Device? _deviceAt(Offset p) {
    Device? best;
    var bestDist = 72.0;
    for (final d in widget.devices) {
      final pos = _positions[d.id];
      if (pos == null) continue;
      final dist = (pos - p).distance;
      if (dist < bestDist) {
        best = d;
        bestDist = dist;
      }
    }
    return best;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = S.of(context);
    final radar = LayoutBuilder(
      builder: (context, constraints) {
        final size = constraints.biggest;
        _positions = _layout(size);
        final center = Offset(size.width / 2, size.height / 2);
        return Stack(
          clipBehavior: Clip.none,
          children: [
            Positioned.fill(
              child: CustomPaint(
                painter: RadarPainter(
                  sweep: _sweep,
                  pulse: _pulse,
                  accent: theme.colorScheme.primary,
                  ring: theme.colorScheme.outlineVariant,
                ),
              ),
            ),
            Positioned(
              left: center.dx - 40,
              top: center.dy - 40,
              width: 80,
              height: 80,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  OsAvatar(os: widget.selfOs, size: 52, glow: true),
                  const SizedBox(height: 4),
                  Text(s.you, style: theme.textTheme.labelMedium),
                ],
              ),
            ),
            for (final d in widget.devices)
              AnimatedPositioned(
                key: ValueKey(d.id),
                duration: const Duration(milliseconds: 600),
                curve: Curves.easeOutCubic,
                left: _positions[d.id]!.dx - _nodeW / 2,
                top: _positions[d.id]!.dy - 26,
                width: _nodeW,
                height: _nodeH,
                child: _DeviceNode(
                  device: d,
                  highlighted: _hoverId == d.id,
                  onTap: () => widget.onTap(d),
                  onLongPress: () => widget.onLongPress(d),
                ),
              ),
            if (widget.devices.isEmpty)
              Positioned(
                left: 24,
                right: 24,
                bottom: 12,
                child: Column(
                  children: [
                    Text(s.searching, style: theme.textTheme.titleSmall, textAlign: TextAlign.center),
                    const SizedBox(height: 4),
                    Text(
                      s.searchingHint,
                      style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.onSurfaceVariant),
                      textAlign: TextAlign.center,
                    ),
                  ],
                ),
              ),
            if (_dragging)
              Positioned.fill(
                child: IgnorePointer(
                  child: Container(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(28),
                      border: Border.all(color: theme.colorScheme.primary, width: 2),
                      color: theme.colorScheme.primary.withValues(alpha: 0.06),
                    ),
                    alignment: Alignment.topCenter,
                    padding: const EdgeInsets.only(top: 12),
                    child: Chip(avatar: const Icon(Icons.file_download_outlined, size: 18), label: Text(s.dropHere)),
                  ),
                ),
              ),
          ],
        );
      },
    );

    if (!isDesktop) return radar;
    return DropTarget(
      onDragEntered: (_) => setState(() => _dragging = true),
      onDragExited: (_) => setState(() {
        _dragging = false;
        _hoverId = null;
      }),
      onDragUpdated: (details) {
        final id = _deviceAt(details.localPosition)?.id;
        if (id != _hoverId) setState(() => _hoverId = id);
      },
      onDragDone: (details) {
        final target = _deviceAt(details.localPosition);
        setState(() {
          _dragging = false;
          _hoverId = null;
        });
        final paths = details.files.map((f) => f.path).where((p) => p.isNotEmpty).toList();
        if (paths.isNotEmpty) widget.onDrop(paths, target);
      },
      child: radar,
    );
  }
}

class _DeviceNode extends StatelessWidget {
  const _DeviceNode({required this.device, required this.highlighted, required this.onTap, required this.onLongPress});

  final Device device;
  final bool highlighted;
  final VoidCallback onTap;
  final VoidCallback onLongPress;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0.4, end: 1),
      duration: const Duration(milliseconds: 500),
      curve: Curves.easeOutBack,
      builder: (context, v, child) => Opacity(
        opacity: v.clamp(0.0, 1.0),
        child: Transform.scale(scale: v, child: child),
      ),
      child: Opacity(
        opacity: device.reachable ? 1 : 0.75,
        child: InkWell(
          borderRadius: BorderRadius.circular(16),
          onTap: onTap,
          onLongPress: onLongPress,
          onSecondaryTap: onLongPress,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              AnimatedScale(
                scale: highlighted ? 1.25 : 1,
                duration: const Duration(milliseconds: 180),
                child: Stack(
                  clipBehavior: Clip.none,
                  children: [
                    OsAvatar(os: device.os, size: 50, glow: highlighted),
                    if (device.trusted)
                      Positioned(
                        right: -4,
                        bottom: -2,
                        child: Icon(Icons.verified, size: 18, color: theme.colorScheme.primary),
                      ),
                  ],
                ),
              ),
              const SizedBox(height: 4),
              Text(device.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: theme.textTheme.labelLarge),
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final t in {for (final t in device.transports) transportIcon(t)})
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 2),
                      child: Icon(t, size: 12, color: theme.colorScheme.onSurfaceVariant),
                    ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class RadarPainter extends CustomPainter {
  RadarPainter({required this.sweep, required this.pulse, required this.accent, required this.ring})
    : super(repaint: Listenable.merge([sweep, pulse]));

  final Animation<double> sweep;
  final Animation<double> pulse;
  final Color accent;
  final Color ring;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = math.max(60.0, math.min(size.width, size.height) / 2 - 46);

    canvas.drawCircle(
      center,
      radius * 1.05,
      Paint()
        ..shader = RadialGradient(colors: [accent.withValues(alpha: 0.10), accent.withValues(alpha: 0.0)])
            .createShader(Rect.fromCircle(center: center, radius: radius * 1.05)),
    );

    final ringPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1
      ..color = ring.withValues(alpha: 0.45);
    for (final f in [0.3, 0.55, 0.8, 1.0]) {
      canvas.drawCircle(center, radius * f, ringPaint);
    }
    final axis = Paint()
      ..strokeWidth = 1
      ..color = ring.withValues(alpha: 0.18);
    canvas.drawLine(center - Offset(radius, 0), center + Offset(radius, 0), axis);
    canvas.drawLine(center - Offset(0, radius), center + Offset(0, radius), axis);

    // Rotating sweep beam.
    const beam = math.pi / 2.6;
    final angle = sweep.value * 2 * math.pi;
    final rect = Rect.fromCircle(center: center, radius: radius);
    canvas.drawArc(
      rect,
      angle - beam,
      beam,
      true,
      Paint()
        ..shader = SweepGradient(
          startAngle: 0,
          endAngle: beam,
          colors: [accent.withValues(alpha: 0.0), accent.withValues(alpha: 0.32)],
          transform: GradientRotation(angle - beam),
        ).createShader(rect),
    );
    canvas.drawLine(
      center,
      center + Offset(math.cos(angle) * radius, math.sin(angle) * radius),
      Paint()
        ..strokeWidth = 1.5
        ..color = accent.withValues(alpha: 0.6),
    );

    // Pulse around this device.
    for (final phase in [0.0, 0.5]) {
      final p = (pulse.value + phase) % 1.0;
      canvas.drawCircle(
        center,
        28 + p * radius * 0.45,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2
          ..color = accent.withValues(alpha: (1 - p) * 0.35),
      );
    }
  }

  @override
  bool shouldRepaint(RadarPainter oldDelegate) => oldDelegate.accent != accent || oldDelegate.ring != ring;
}
