import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:permission_handler/permission_handler.dart';

import '../core/controller.dart';
import '../core/models.dart';
import '../l10n.dart';
import 'file_browser.dart';
import 'widgets.dart';
import 'wifi_direct_sheet.dart';

/// Entry point when the user taps a device on the radar.
Future<void> openDevice(BuildContext context, Device device) async {
  final c = OmniScope.read(context);
  if (!device.reachable) {
    await showOffGridSheet(context, device);
    return;
  }
  await showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    isScrollControlled: true,
    builder: (_) => OmniScope(
      controller: c,
      child: _SendSheet(device: device),
    ),
  );
}

Future<void> showOffGridSheet(BuildContext context, Device device) async {
  final c = OmniScope.read(context);
  final s = S.of(context);
  await showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    builder: (sheetContext) => Padding(
      padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          OsAvatar(os: device.os, size: 56),
          const SizedBox(height: 12),
          Text(device.name, style: Theme.of(sheetContext).textTheme.titleLarge),
          const SizedBox(height: 4),
          Text(s.notReachable, textAlign: TextAlign.center),
          const SizedBox(height: 20),
          FilledButton.icon(
            icon: const Icon(Icons.wifi_tethering),
            label: Text(s.connectWifiDirect),
            onPressed: () async {
              Navigator.pop(sheetContext);
              final direct = await c.connectOffGrid(device);
              if (!direct && context.mounted) await showWifiDirectSheet(context);
            },
          ),
        ],
      ),
    ),
  );
}

/// Desktop drag-and-drop or Android share: pick a target, then send.
Future<void> sendPathsInteractive(BuildContext context, List<String> paths, {Device? target, String? text}) async {
  final c = OmniScope.read(context);
  var device = target;
  device ??= await pickDevice(context, c.devices.where((d) => d.reachable).toList());
  if (device == null || !context.mounted) return;
  await startSend(context, device, paths, text: text);
}

Future<Device?> pickDevice(BuildContext context, List<Device> devices) async {
  final s = S.of(context);
  if (devices.length == 1) return devices.first;
  return showDialog<Device>(
    context: context,
    builder: (dialogContext) => SimpleDialog(
      title: Text(s.chooseDevice),
      children: [
        if (devices.isEmpty) Padding(padding: const EdgeInsets.all(24), child: Text(s.searching)),
        for (final d in devices)
          SimpleDialogOption(
            onPressed: () => Navigator.pop(dialogContext, d),
            child: ListTile(
              leading: OsAvatar(os: d.os, size: 36),
              title: Text(d.name),
              subtitle: Text(d.addresses.isEmpty ? '' : d.addresses.first),
            ),
          ),
      ],
    ),
  );
}

Future<void> startSend(BuildContext context, Device device, List<String> paths, {String? text}) async {
  final c = OmniScope.read(context);
  try {
    final id = await c.send(device, paths, text: text);
    if (!context.mounted) return;
    await showDialog<void>(
      context: context,
      barrierDismissible: true,
      builder: (_) => OmniScope(
        controller: c,
        child: _WaitingDialog(transferId: id),
      ),
    );
  } on StateError catch (e) {
    if (!context.mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e.message)));
  }
}

class _SendSheet extends StatelessWidget {
  const _SendSheet({required this.device});

  final Device device;

  Future<void> _pickFiles(BuildContext context) async {
    final files = await openFiles();
    if (files.isEmpty || !context.mounted) return;
    Navigator.pop(context);
    await startSend(context, device, files.map((f) => f.path).toList());
  }

  Future<void> _pickFolder(BuildContext context) async {
    final dir = await getDirectoryPath();
    if (dir == null || !context.mounted) return;
    Navigator.pop(context);
    await startSend(context, device, [dir]);
  }

  Future<void> _browse(BuildContext context) async {
    final s = S.of(context);
    var granted = await Permission.manageExternalStorage.isGranted || await Permission.storage.isGranted;
    if (!granted) {
      granted =
          (await Permission.manageExternalStorage.request()).isGranted ||
          (await Permission.storage.request()).isGranted;
    }
    if (!context.mounted) return;
    if (!granted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(s.storageDenied)));
      return;
    }
    final c = OmniScope.read(context);
    final root = await c.host.externalStorageRoot() ?? '/storage/emulated/0';
    if (!context.mounted) return;
    final paths = await Navigator.push<List<String>>(
      context,
      MaterialPageRoute(
        builder: (_) => FileBrowser(root: root, mode: BrowserMode.send),
      ),
    );
    if (paths == null || paths.isEmpty || !context.mounted) return;
    Navigator.pop(context);
    await startSend(context, device, paths);
  }

  Future<void> _clipboard(BuildContext context) async {
    final s = S.of(context);
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text?.trim() ?? '';
    if (!context.mounted) return;
    if (text.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(s.clipboardEmpty)));
      return;
    }
    Navigator.pop(context);
    await startSend(context, device, const [], text: text);
  }

  Future<void> _text(BuildContext context) async {
    final s = S.of(context);
    final controller = TextEditingController();
    final text = await showDialog<String>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(s.textOrLink),
        content: TextField(
          controller: controller,
          autofocus: true,
          minLines: 2,
          maxLines: 6,
          decoration: InputDecoration(hintText: s.typeTextHint, border: const OutlineInputBorder()),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(dialogContext), child: Text(s.cancel)),
          FilledButton(onPressed: () => Navigator.pop(dialogContext, controller.text.trim()), child: Text(s.send)),
        ],
      ),
    );
    if (text == null || text.isEmpty || !context.mounted) return;
    Navigator.pop(context);
    await startSend(context, device, const [], text: text);
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final actions = <(IconData, String, Future<void> Function(BuildContext))>[
      if (Platform.isAndroid) (Icons.folder_open, s.browseStorage, _browse),
      (Icons.insert_drive_file_outlined, Platform.isAndroid ? s.systemPicker : s.files, _pickFiles),
      if (!Platform.isAndroid) (Icons.folder_outlined, s.folder, _pickFolder),
      (Icons.content_paste, s.clipboard, _clipboard),
      (Icons.short_text, s.textOrLink, _text),
    ];
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                OsAvatar(os: device.os, size: 48),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(s.sendTo(device.name), style: theme.textTheme.titleLarge),
                      Wrap(
                        spacing: 6,
                        children: [
                          for (final t in device.transports)
                            Text(
                              transportLabel(t),
                              style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.onSurfaceVariant),
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 20),
            GridView.count(
              shrinkWrap: true,
              physics: const NeverScrollableScrollPhysics(),
              crossAxisCount: actions.length > 4 ? 3 : 4,
              mainAxisSpacing: 12,
              crossAxisSpacing: 12,
              childAspectRatio: 1.05,
              children: [
                for (final (icon, label, action) in actions)
                  Material(
                    color: theme.colorScheme.surfaceContainerHigh,
                    borderRadius: BorderRadius.circular(18),
                    child: InkWell(
                      borderRadius: BorderRadius.circular(18),
                      onTap: () => action(context),
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          Icon(icon, size: 30, color: theme.colorScheme.primary),
                          const SizedBox(height: 8),
                          Text(label, textAlign: TextAlign.center, style: theme.textTheme.labelLarge),
                        ],
                      ),
                    ),
                  ),
              ],
            ),
            if (isDesktopPlatform) ...[
              const SizedBox(height: 14),
              Text(
                s.noTransfersHint,
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.onSurfaceVariant),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

bool get isDesktopPlatform => Platform.isWindows || Platform.isLinux;

/// Shown on the sender while the receiver decides: displays the SAS to compare.
class _WaitingDialog extends StatelessWidget {
  const _WaitingDialog({required this.transferId});

  final String transferId;

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.of(context);
    final s = S.of(context);
    final t = c.transfers[transferId];
    final waiting = t == null || const {'preparing', 'connecting', 'waiting_accept'}.contains(t.state);
    if (t != null && !waiting) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!context.mounted) return;
        Navigator.of(context).maybePop();
        if (t.state == 'rejected' || t.state == 'failed') {
          ScaffoldMessenger.of(context)
              .showSnackBar(SnackBar(content: Text('${s.state(t.state)}${t.error != null ? ': ${t.error}' : ''}')));
        }
      });
    }
    return AlertDialog(
      icon: t == null ? null : OsAvatar(os: t.peer.os, size: 52),
      title: Text(t == null ? s.state('preparing') : s.waitingFor(t.peer.name), textAlign: TextAlign.center),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (t?.sas != null) ...[
            SasView(sas: t!.sas!),
            const SizedBox(height: 14),
            Text(s.compareCode, textAlign: TextAlign.center),
          ] else
            const Padding(padding: EdgeInsets.all(16), child: CircularProgressIndicator()),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () {
            c.cancel(transferId);
            Navigator.pop(context);
          },
          child: Text(s.cancel),
        ),
      ],
    );
  }
}
