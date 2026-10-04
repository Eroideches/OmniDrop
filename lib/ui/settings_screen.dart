import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../core/controller.dart';
import '../l10n.dart';
import 'file_browser.dart';
import 'permissions_screen.dart';
import 'widgets.dart';

class SettingsScreen extends StatefulWidget {
  const SettingsScreen({super.key});

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  final _name = TextEditingController();
  bool _nameInit = false;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _apply(OmniController c, Map<String, dynamic> patch) {
    final error = c.updateSettings(patch);
    if (error != null) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(error)));
    }
  }

  Future<void> _changeFolder(OmniController c) async {
    String? dir;
    if (Platform.isAndroid) {
      final root = await c.host.externalStorageRoot() ?? '/storage/emulated/0';
      if (!mounted) return;
      final picked = await Navigator.push<List<String>>(
        context,
        MaterialPageRoute(
          builder: (_) => FileBrowser(root: root, mode: BrowserMode.pickFolder),
        ),
      );
      dir = picked?.firstOrNull;
    } else {
      dir = await getDirectoryPath(initialDirectory: c.settings.downloadDir);
    }
    if (dir != null) _apply(c, {'download_dir': dir});
  }

  Future<void> _addByIp(OmniController c) async {
    final s = S.of(context);
    final ctrl = TextEditingController();
    final ip = await showDialog<String>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(s.addByIp),
        content: TextField(
          controller: ctrl,
          autofocus: true,
          keyboardType: TextInputType.url,
          decoration: InputDecoration(hintText: s.ipAddress, border: const OutlineInputBorder()),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(dialogContext), child: Text(s.cancel)),
          FilledButton(onPressed: () => Navigator.pop(dialogContext, ctrl.text.trim()), child: Text(s.add)),
        ],
      ),
    );
    if (ip != null && ip.isNotEmpty) c.probe(ip);
  }

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.of(context);
    final s = S.of(context);
    final theme = Theme.of(context);
    final st = c.settings;
    if (!_nameInit && st.deviceName.isNotEmpty) {
      _name.text = st.deviceName;
      _nameInit = true;
    }
    final fingerprint = c.publicKey.isEmpty
        ? ''
        : RegExp(r'.{1,4}').allMatches(c.publicKey.substring(0, 32).toUpperCase()).map((m) => m[0]).join(' ');
    Widget section(String title) => Padding(
      padding: const EdgeInsets.fromLTRB(16, 20, 16, 6),
      child: Text(title, style: theme.textTheme.titleSmall?.copyWith(color: theme.colorScheme.primary)),
    );
    return Scaffold(
      appBar: AppBar(title: Text(s.settings)),
      body: ListView(
        padding: const EdgeInsets.only(bottom: 32),
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
            child: TextField(
              controller: _name,
              decoration: InputDecoration(
                labelText: s.deviceName,
                prefixIcon: Icon(osIcon(c.os)),
                border: const OutlineInputBorder(),
              ),
              maxLength: 64,
              onSubmitted: (v) => _apply(c, {'device_name': v}),
              onTapOutside: (_) {
                if (_name.text.trim() != st.deviceName && _name.text.trim().isNotEmpty) {
                  _apply(c, {'device_name': _name.text.trim()});
                }
                FocusScope.of(context).unfocus();
              },
            ),
          ),
          ListTile(
            leading: const Icon(Icons.download_outlined),
            title: Text(s.downloadFolder),
            subtitle: Text(st.downloadDir),
            trailing: TextButton(onPressed: () => _changeFolder(c), child: Text(s.change)),
          ),
          section(s.discovery),
          SwitchListTile(
            secondary: const Icon(Icons.travel_explore),
            title: Text(s.mdnsTitle),
            subtitle: Text(s.mdnsSubtitle),
            value: st.mdns,
            onChanged: (v) => _apply(c, {'mdns': v}),
          ),
          SwitchListTile(
            secondary: const Icon(Icons.cell_tower),
            title: Text(s.broadcastTitle),
            subtitle: Text(s.broadcastSubtitle),
            value: st.broadcast,
            onChanged: (v) => _apply(c, {'broadcast': v}),
          ),
          SwitchListTile(
            secondary: const Icon(Icons.bluetooth),
            title: Text(s.bleTitle),
            subtitle: Text(c.bleError ?? s.bleSubtitle),
            value: st.ble,
            onChanged: (v) => _apply(c, {'ble': v}),
          ),
          if (c.wifiDirectSupported)
            SwitchListTile(
              secondary: const Icon(Icons.wifi_tethering),
              title: Text(s.wifiDirect),
              subtitle: Text(s.wifiDirectSubtitle),
              value: st.wifiDirect,
              onChanged: (v) => _apply(c, {'wifi_direct': v}),
            ),
          ListTile(leading: const Icon(Icons.add_link), title: Text(s.addByIp), onTap: () => _addByIp(c)),
          section(s.transferSection),
          ListTile(
            leading: const Icon(Icons.call_split),
            title: Text(s.parallelStreams),
            subtitle: Slider(
              value: st.streams.toDouble(),
              min: 1,
              max: 8,
              divisions: 7,
              label: '${st.streams}',
              onChanged: (v) => _apply(c, {'streams': v.round()}),
            ),
            trailing: Text('${st.streams}', style: theme.textTheme.titleMedium),
          ),
          SwitchListTile(
            secondary: const Icon(Icons.verified_user_outlined),
            title: Text(s.autoAccept),
            value: st.autoAcceptTrusted,
            onChanged: (v) => _apply(c, {'auto_accept_trusted': v}),
          ),
          section(s.trustedDevices),
          if (st.trusted.isEmpty)
            ListTile(leading: const Icon(Icons.shield_outlined), title: Text(s.noTrusted))
          else
            for (final t in st.trusted)
              ListTile(
                leading: OsAvatar(os: t.os, size: 36),
                title: Text(t.name),
                subtitle: Text(t.id, style: const TextStyle(fontFamily: 'monospace')),
                trailing: IconButton(
                  icon: const Icon(Icons.delete_outline),
                  tooltip: s.remove,
                  onPressed: () => c.forgetTrusted(t.id),
                ),
              ),
          if (Platform.isAndroid) ...[
            section(s.permissions),
            ListTile(
              leading: const Icon(Icons.admin_panel_settings_outlined),
              title: Text(s.permissions),
              subtitle: Text(s.permissionsIntro, maxLines: 2, overflow: TextOverflow.ellipsis),
              onTap: () => Navigator.push(
                context,
                MaterialPageRoute(
                  builder: (_) => OmniScope(controller: c, child: const PermissionsScreen()),
                ),
              ),
            ),
          ],
          section(s.thisDevice),
          ListTile(
            leading: const Icon(Icons.fingerprint),
            title: Text(s.fingerprint),
            subtitle: SelectableText(fingerprint, style: const TextStyle(fontFamily: 'monospace')),
            trailing: IconButton(
              icon: const Icon(Icons.copy),
              onPressed: () => Clipboard.setData(ClipboardData(text: c.publicKey)),
            ),
          ),
          ListTile(
            leading: const Icon(Icons.badge_outlined),
            title: Text(s.deviceId),
            subtitle: SelectableText(c.deviceId, style: const TextStyle(fontFamily: 'monospace')),
          ),
          ListTile(
            leading: const Icon(Icons.lan_outlined),
            title: Text(s.addresses),
            subtitle: Text([...c.localAddresses, 'TCP ${c.port}'].join('\n')),
          ),
          ListTile(
            leading: const Icon(Icons.lock_outline),
            title: Text(s.security),
            subtitle: Text('${s.securityInfo}\n${s.aesHw}: ${c.aesHw ? s.yes : s.no} · ${s.ioBackend}: ${c.ioBackend}'),
          ),
          if (Platform.isWindows) ListTile(leading: const Icon(Icons.security), title: Text(s.firewallHint)),
          ListTile(
            leading: const Icon(Icons.info_outline),
            title: Text('${s.appName} 1.0.0'),
            subtitle: Text(s.license),
          ),
        ],
      ),
    );
  }
}
