import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../core/controller.dart';
import '../core/format.dart';
import '../core/models.dart';
import '../l10n.dart';
import 'widgets.dart';

/// Transfer Monitor: live speed chart, progress, ETA, block map, pause/cancel.
class TransfersScreen extends StatelessWidget {
  const TransfersScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.of(context);
    final s = S.of(context);
    final list = c.transfers.values.toList()..sort((a, b) => b.startedMs.compareTo(a.startedMs));
    return Scaffold(
      appBar: AppBar(
        title: Text(s.transfers),
        actions: [
          if (list.any((t) => t.terminal))
            TextButton.icon(
              onPressed: c.clearFinished,
              icon: const Icon(Icons.clear_all),
              label: Text(s.clearFinished),
            ),
        ],
      ),
      body: list.isEmpty
          ? EmptyState(icon: Icons.swap_vert_circle_outlined, title: s.noTransfers, subtitle: s.noTransfersHint)
          : ListView.builder(
              padding: const EdgeInsets.fromLTRB(12, 4, 12, 24),
              itemCount: list.length,
              itemBuilder: (_, i) => TransferCard(transfer: list[i], history: c.speedHistory[list[i].id] ?? const []),
            ),
    );
  }
}

class TransferCard extends StatelessWidget {
  const TransferCard({super.key, required this.transfer, required this.history});

  final TransferInfo transfer;
  final List<double> history;

  Color _stateColor(ThemeData theme) {
    switch (transfer.state) {
      case 'completed':
        return const Color(0xFF3DDC84);
      case 'failed':
      case 'rejected':
        return theme.colorScheme.error;
      case 'cancelled':
      case 'interrupted':
        return theme.colorScheme.outline;
      default:
        return theme.colorScheme.primary;
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.read(context);
    final s = S.of(context);
    final theme = Theme.of(context);
    final t = transfer;
    final color = _stateColor(theme);
    final percent = (t.progress * 100).floor();
    final stateLabel = t.paused && t.canPause ? s.paused : s.state(t.state);
    final showChart = !t.terminal && t.totalBytes > 0 || history.length > 3;
    return Card(
      margin: const EdgeInsets.symmetric(vertical: 6),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                OsAvatar(os: t.peer.os, size: 42),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Icon(t.outgoing ? Icons.north_east : Icons.south_west, size: 16, color: color),
                          const SizedBox(width: 4),
                          Flexible(
                            child: Text(
                              '${t.outgoing ? s.sending : s.receiving} ${t.peer.name}',
                              style: theme.textTheme.titleMedium,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                        ],
                      ),
                      const SizedBox(height: 2),
                      Text(stateLabel, style: theme.textTheme.labelMedium?.copyWith(color: color)),
                    ],
                  ),
                ),
                if (t.totalBytes > 0)
                  Text(
                    '$percent%',
                    style: theme.textTheme.headlineSmall?.copyWith(
                      fontWeight: FontWeight.w700,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
              ],
            ),
            if (t.outgoing && t.state == 'waiting_accept' && t.sas != null) ...[
              const SizedBox(height: 12),
              Center(child: SasView(sas: t.sas!, compact: true)),
              const SizedBox(height: 6),
              Text(s.compareCode, textAlign: TextAlign.center, style: theme.textTheme.bodySmall),
            ],
            if (t.totalBytes > 0) ...[
              const SizedBox(height: 14),
              ClipRRect(
                borderRadius: BorderRadius.circular(8),
                child: LinearProgressIndicator(
                  value: t.progress,
                  minHeight: 8,
                  color: color,
                  backgroundColor: theme.colorScheme.surfaceContainerHighest,
                ),
              ),
              const SizedBox(height: 8),
              Wrap(
                spacing: 14,
                runSpacing: 4,
                children: [
                  _Stat(Icons.data_usage, '${formatBytes(t.doneBytes)} / ${formatBytes(t.totalBytes)}'),
                  if (t.state == 'transferring' && !t.paused) _Stat(Icons.speed, formatSpeed(t.speed)),
                  if (t.eta != null && t.state == 'transferring' && !t.paused)
                    _Stat(Icons.timer_outlined, s.eta(formatDuration(t.eta!))),
                  if (t.streams > 0) _Stat(Icons.call_split, s.streams(t.streams)),
                  if (t.cipher != null) _Stat(Icons.lock_outline, t.cipher!),
                ],
              ),
            ] else if (t.text != null && t.text!.isNotEmpty) ...[
              const SizedBox(height: 10),
              Text(t.text!, maxLines: 3, overflow: TextOverflow.ellipsis),
            ],
            if (showChart && t.totalBytes > 0) ...[
              const SizedBox(height: 12),
              SizedBox(
                height: 70,
                child: SpeedChart(samples: history, color: color),
              ),
            ],
            if (t.blocksTotal > 0) ...[
              const SizedBox(height: 12),
              Row(
                children: [
                  Text(s.blocks(t.blocksDone, t.blocksTotal), style: theme.textTheme.labelSmall),
                  const Spacer(),
                  if (t.currentFile != null && !t.terminal)
                    Flexible(
                      child: Text(
                        basename(t.currentFile!),
                        style: theme.textTheme.labelSmall,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
              ),
              const SizedBox(height: 6),
              BlockMap(map: t.blockMap, color: color),
            ],
            if (t.fileCount > 1)
              Theme(
                data: theme.copyWith(dividerColor: Colors.transparent),
                child: ExpansionTile(
                  tilePadding: EdgeInsets.zero,
                  title: Text(s.filesCount(t.fileCount), style: theme.textTheme.labelLarge),
                  children: [
                    for (final f in t.files)
                      ListTile(
                        dense: true,
                        contentPadding: EdgeInsets.zero,
                        leading: Icon(
                          f.failed
                              ? Icons.error_outline
                              : f.done
                              ? Icons.check_circle
                              : fileKindIcon(kindOf(f.path)),
                          size: 20,
                          color: f.failed ? theme.colorScheme.error : (f.done ? const Color(0xFF3DDC84) : null),
                        ),
                        title: Text(f.path, maxLines: 1, overflow: TextOverflow.ellipsis),
                        trailing: Text(formatBytes(f.size)),
                      ),
                  ],
                ),
              ),
            if (t.error != null && t.state != 'completed') ...[
              const SizedBox(height: 8),
              Text(t.error!, style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.error)),
            ],
            if (!t.outgoing && t.state == 'completed' && t.savedPaths.isNotEmpty) ...[
              const SizedBox(height: 6),
              Text(
                s.savedIn(t.destDir),
                style: theme.textTheme.bodySmall,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            ],
            const SizedBox(height: 8),
            Wrap(
              alignment: WrapAlignment.end,
              spacing: 8,
              children: [
                if (t.canPause || (t.paused && !t.terminal))
                  t.localPaused
                      ? FilledButton.tonalIcon(
                          onPressed: () => c.resume(t.id),
                          icon: const Icon(Icons.play_arrow),
                          label: Text(s.resume),
                        )
                      : FilledButton.tonalIcon(
                          onPressed: () => c.pause(t.id),
                          icon: const Icon(Icons.pause),
                          label: Text(s.pause),
                        ),
                if (!t.terminal)
                  OutlinedButton.icon(
                    onPressed: () => c.cancel(t.id),
                    icon: const Icon(Icons.close),
                    label: Text(s.cancel),
                  ),
                if (!t.outgoing && t.state == 'completed' && t.savedPaths.length == 1)
                  FilledButton.icon(
                    onPressed: () => c.host.openFile(t.savedPaths.first),
                    icon: const Icon(Icons.open_in_new),
                    label: Text(s.open),
                  ),
                if (!t.outgoing && t.state == 'completed' && t.savedPaths.isNotEmpty)
                  OutlinedButton.icon(
                    onPressed: () => c.host.revealInFolder(t.savedPaths.length == 1 ? t.savedPaths.first : t.destDir),
                    icon: const Icon(Icons.folder_open),
                    label: Text(s.openFolder),
                  ),
                if (t.terminal) TextButton(onPressed: () => c.removeTransfer(t.id), child: Text(s.remove)),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _Stat extends StatelessWidget {
  const _Stat(this.icon, this.text);

  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 14, color: theme.colorScheme.onSurfaceVariant),
        const SizedBox(width: 4),
        Text(text, style: theme.textTheme.bodySmall?.copyWith(fontFeatures: const [FontFeature.tabularFigures()])),
      ],
    );
  }
}

/// Real-time throughput area chart (MB/s samples at ~5 Hz).
class SpeedChart extends StatelessWidget {
  const SpeedChart({super.key, required this.samples, required this.color});

  final List<double> samples;
  final Color color;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return CustomPaint(
      painter: _SpeedPainter(
        samples: List.of(samples),
        color: color,
        grid: theme.colorScheme.outlineVariant,
        label: theme.textTheme.labelSmall!.copyWith(color: theme.colorScheme.onSurfaceVariant),
      ),
    );
  }
}

class _SpeedPainter extends CustomPainter {
  _SpeedPainter({required this.samples, required this.color, required this.grid, required this.label});

  final List<double> samples;
  final Color color;
  final Color grid;
  final TextStyle label;
  static const _window = 150;

  @override
  void paint(Canvas canvas, Size size) {
    final peak = samples.isEmpty ? 1.0 : samples.reduce(math.max);
    final top = peak <= 0 ? 1.0 : peak * 1.2;
    final gridPaint = Paint()
      ..color = grid.withValues(alpha: 0.4)
      ..strokeWidth = 1;
    for (final f in [0.0, 0.5, 1.0]) {
      final y = size.height * f;
      canvas.drawLine(Offset(0, y), Offset(size.width, y), gridPaint);
    }
    final tp = TextPainter(
      text: TextSpan(text: '${peak.toStringAsFixed(peak >= 10 ? 0 : 1)} MB/s', style: label),
      textDirection: TextDirection.ltr,
    )..layout();
    tp.paint(canvas, Offset(size.width - tp.width, 0));
    if (samples.length < 2) return;
    final dx = size.width / (_window - 1);
    final start = _window - samples.length;
    final path = Path();
    for (var i = 0; i < samples.length; i++) {
      final x = (start + i) * dx;
      final y = size.height - (samples[i] / top) * size.height;
      if (i == 0) {
        path.moveTo(x, y);
      } else {
        path.lineTo(x, y);
      }
    }
    final fill = Path.from(path)
      ..lineTo((start + samples.length - 1) * dx, size.height)
      ..lineTo(start * dx, size.height)
      ..close();
    canvas.drawPath(
      fill,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [color.withValues(alpha: 0.35), color.withValues(alpha: 0.02)],
        ).createShader(Offset.zero & size),
    );
    canvas.drawPath(
      path,
      Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..strokeJoin = StrokeJoin.round,
    );
  }

  @override
  bool shouldRepaint(_SpeedPainter old) => true;
}

/// Block-level progress: each cell aggregates a run of 512 KiB blocks
/// (0 = pending, 1 = partially done / in flight, 2 = verified).
class BlockMap extends StatelessWidget {
  const BlockMap({super.key, required this.map, required this.color});

  final String map;
  final Color color;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        const cell = 9.0;
        const gap = 2.0;
        final cols = math.max(1, ((constraints.maxWidth + gap) / (cell + gap)).floor());
        final rows = (map.length / cols).ceil();
        return SizedBox(
          height: rows * (cell + gap),
          width: constraints.maxWidth,
          child: CustomPaint(
            painter: _BlockPainter(
              map: map,
              cols: cols,
              done: color,
              partial: color.withValues(alpha: 0.45),
              pending: theme.colorScheme.surfaceContainerHighest,
            ),
          ),
        );
      },
    );
  }
}

class _BlockPainter extends CustomPainter {
  _BlockPainter({
    required this.map,
    required this.cols,
    required this.done,
    required this.partial,
    required this.pending,
  });

  final String map;
  final int cols;
  final Color done;
  final Color partial;
  final Color pending;

  @override
  void paint(Canvas canvas, Size size) {
    const cell = 9.0;
    const gap = 2.0;
    final paints = {'0': Paint()..color = pending, '1': Paint()..color = partial, '2': Paint()..color = done};
    for (var i = 0; i < map.length; i++) {
      final x = (i % cols) * (cell + gap);
      final y = (i ~/ cols) * (cell + gap);
      canvas.drawRRect(
        RRect.fromRectAndRadius(Rect.fromLTWH(x, y, cell, cell), const Radius.circular(2)),
        paints[map[i]] ?? paints['0']!,
      );
    }
  }

  @override
  bool shouldRepaint(_BlockPainter old) => old.map != map || old.cols != cols || old.done != done;
}
