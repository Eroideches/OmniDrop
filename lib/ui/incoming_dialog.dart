import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../core/controller.dart';
import '../core/format.dart';
import '../core/models.dart';
import '../l10n.dart';
import 'widgets.dart';

/// Incoming offer: sender, file preview and the SAS to compare before accepting.
Future<void> showIncomingDialog(BuildContext context, IncomingRequest req) async {
  final c = OmniScope.read(context);
  final result = await showDialog<(bool, bool)>(
    context: context,
    barrierDismissible: false,
    builder: (_) => OmniScope(
      controller: c,
      child: _IncomingDialog(req: req),
    ),
  );
  if (result == null) return;
  final ok = c.respond(req.requestId, accept: result.$1, trust: result.$2);
  if (!ok && context.mounted) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(S.of(context).requestExpired)));
  }
}

/// Thumbnails come from the remote device: ignore anything that is not valid base64.
Uint8List? _decodeThumb(String data) {
  try {
    return base64Decode(data);
  } on FormatException {
    return null;
  }
}

class _IncomingDialog extends StatefulWidget {
  const _IncomingDialog({required this.req});

  final IncomingRequest req;

  @override
  State<_IncomingDialog> createState() => _IncomingDialogState();
}

class _IncomingDialogState extends State<_IncomingDialog> {
  bool _trust = false;
  StreamSubscription<String>? _cancelSub;

  @override
  void initState() {
    super.initState();
    _trust = widget.req.trusted;
    _cancelSub = OmniScope.read(context).incomingCancelled.listen((id) {
      if (id == widget.req.requestId && mounted) {
        Navigator.pop(context);
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(S.of(context).incomingCancelled)));
      }
    });
  }

  @override
  void dispose() {
    _cancelSub?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final req = widget.req;
    final files = req.items.where((i) => !i.dir).toList();
    final shownFiles = files.take(5).toList();
    final what = <String>[
      if (req.fileCount > 0) '${s.filesCount(req.fileCount)} · ${formatBytes(req.totalSize)}',
      if (req.text != null && req.text!.isNotEmpty) s.aMessage,
    ].join(' + ');
    return AlertDialog(
      scrollable: true,
      icon: OsAvatar(os: req.peer.os, size: 56, glow: true),
      title: Column(
        children: [
          Text(s.incomingTitle, style: theme.textTheme.labelLarge?.copyWith(color: theme.colorScheme.primary)),
          const SizedBox(height: 4),
          Text(s.wantsToSend(req.peer.name), textAlign: TextAlign.center),
          Text(what, style: theme.textTheme.titleMedium, textAlign: TextAlign.center),
          if (req.trusted)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.verified, size: 16, color: theme.colorScheme.primary),
                  const SizedBox(width: 4),
                  Text(s.trustedDevice, style: theme.textTheme.labelMedium),
                ],
              ),
            ),
        ],
      ),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (req.thumbs.isNotEmpty) ...[
              SizedBox(
                height: 84,
                child: ListView(
                  scrollDirection: Axis.horizontal,
                  children: [
                    for (final bytes in req.thumbs.values.map(_decodeThumb).whereType<Uint8List>())
                      Padding(
                        padding: const EdgeInsets.only(right: 8),
                        child: ClipRRect(
                          borderRadius: BorderRadius.circular(12),
                          child: Image.memory(
                            bytes,
                            height: 84,
                            fit: BoxFit.cover,
                            gaplessPlayback: true,
                            errorBuilder: (_, _, _) => const SizedBox.shrink(),
                          ),
                        ),
                      ),
                  ],
                ),
              ),
              const SizedBox(height: 12),
            ],
            for (final f in shownFiles)
              ListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                leading: Icon(fileKindIcon(kindOf(f.path))),
                title: Text(f.path, maxLines: 1, overflow: TextOverflow.ellipsis),
                trailing: Text(formatBytes(f.size)),
              ),
            if (files.length > shownFiles.length || req.fileCount > files.length)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Text(
                  s.moreFiles(req.fileCount - shownFiles.length),
                  style: theme.textTheme.bodySmall,
                  textAlign: TextAlign.center,
                ),
              ),
            if (req.text != null && req.text!.isNotEmpty)
              Container(
                margin: const EdgeInsets.only(bottom: 12),
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: theme.colorScheme.surfaceContainerHigh,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Text(req.text!, maxLines: 6, overflow: TextOverflow.ellipsis),
              ),
            const SizedBox(height: 4),
            Text(s.verifyCode, style: theme.textTheme.labelLarge, textAlign: TextAlign.center),
            const SizedBox(height: 8),
            Center(child: SasView(sas: req.sas)),
            const SizedBox(height: 10),
            Text(
              s.verifyCodeHint(req.peer.name),
              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.onSurfaceVariant),
              textAlign: TextAlign.center,
            ),
            Text(
              req.cipher,
              style: theme.textTheme.labelSmall?.copyWith(color: theme.colorScheme.outline),
              textAlign: TextAlign.center,
            ),
            if (!req.trusted)
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                value: _trust,
                onChanged: (v) => setState(() => _trust = v ?? false),
                title: Text(s.trustDevice),
                controlAffinity: ListTileControlAffinity.leading,
              ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context, (false, false)), child: Text(s.decline)),
        FilledButton(onPressed: () => Navigator.pop(context, (true, _trust)), child: Text(s.accept)),
      ],
    );
  }
}

Future<void> showReceivedText(BuildContext context, ReceivedText msg) async {
  final s = S.of(context);
  final c = OmniScope.read(context);
  await Clipboard.setData(ClipboardData(text: msg.text));
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      icon: OsAvatar(os: msg.from.os, size: 48),
      title: Text(s.messageReceived),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(s.from(msg.from.name), style: Theme.of(dialogContext).textTheme.labelLarge),
          const SizedBox(height: 10),
          SelectableText(msg.text),
          const SizedBox(height: 10),
          Text(s.copied, style: Theme.of(dialogContext).textTheme.bodySmall),
        ],
      ),
      actions: [
        if (looksLikeUrl(msg.text))
          TextButton(
            onPressed: () {
              c.host.openUrl(msg.text);
              Navigator.pop(dialogContext);
            },
            child: Text(s.openLink),
          ),
        FilledButton(onPressed: () => Navigator.pop(dialogContext), child: Text(s.close)),
      ],
    ),
  );
}
