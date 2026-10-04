import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

import '../core/controller.dart';
import '../core/models.dart';
import '../l10n.dart';
import 'incoming_dialog.dart';
import 'permissions_screen.dart';
import 'radar.dart';
import 'send_flow.dart';
import 'settings_screen.dart';
import 'transfers_screen.dart';
import 'widgets.dart';
import 'wifi_direct_sheet.dart';

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key});

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  int _tab = 0;
  final List<StreamSubscription<dynamic>> _subs = [];
  final List<IncomingRequest> _queue = [];
  bool _showingIncoming = false;

  @override
  void initState() {
    super.initState();
    final c = OmniScope.read(context);
    _subs
      ..add(
        c.incoming.listen((req) {
          _queue.add(req);
          _pumpIncoming();
        }),
      )
      ..add(c.incomingCancelled.listen((id) => _queue.removeWhere((r) => r.requestId == id)))
      ..add(
        c.texts.listen((msg) {
          if (mounted) showReceivedText(context, msg);
        }),
      )
      ..add(
        c.notices.listen((text) {
          if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
        }),
      )
      ..add(
        c.shared.listen((content) {
          if (mounted) sendPathsInteractive(context, content.paths, text: content.text);
        }),
      );
    WidgetsBinding.instance.addPostFrameCallback((_) => _maybeOnboard());
  }

  Future<void> _maybeOnboard() async {
    if (!Platform.isAndroid) return;
    final dir = await getApplicationSupportDirectory();
    final flag = File('${dir.path}/onboarding_done');
    if (flag.existsSync() && await PermissionsScreen.essentialsGranted()) return;
    if (!mounted) return;
    final c = OmniScope.read(context);
    await Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => OmniScope(controller: c, child: const PermissionsScreen(onboarding: true)),
      ),
    );
    flag.writeAsStringSync('1');
  }

  Future<void> _pumpIncoming() async {
    if (_showingIncoming) return;
    _showingIncoming = true;
    while (_queue.isNotEmpty && mounted) {
      final req = _queue.removeAt(0);
      await showIncomingDialog(context, req);
    }
    _showingIncoming = false;
  }

  @override
  void dispose() {
    for (final s in _subs) {
      s.cancel();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.of(context);
    final s = S.of(context);
    final wide = MediaQuery.sizeOf(context).width >= 840;
    final active = c.activeTransfers;
    final transfersIcon = Badge(isLabelVisible: active > 0, label: Text('$active'), child: const Icon(Icons.swap_vert));
    final destinations = [
      (const Icon(Icons.radar), s.radar),
      (transfersIcon, s.transfers),
      (const Icon(Icons.settings_outlined), s.settings),
    ];
    final body = IndexedStack(index: _tab, children: const [RadarPage(), TransfersScreen(), SettingsScreen()]);
    if (wide) {
      return Scaffold(
        body: Row(
          children: [
            NavigationRail(
              selectedIndex: _tab,
              onDestinationSelected: (i) => setState(() => _tab = i),
              labelType: NavigationRailLabelType.all,
              leading: Padding(
                padding: const EdgeInsets.symmetric(vertical: 12),
                child: Image.asset('assets/icon.png', width: 40, height: 40),
              ),
              destinations: [
                for (final (icon, label) in destinations) NavigationRailDestination(icon: icon, label: Text(label)),
              ],
            ),
            const VerticalDivider(width: 1),
            Expanded(child: body),
          ],
        ),
      );
    }
    return Scaffold(
      body: body,
      bottomNavigationBar: NavigationBar(
        selectedIndex: _tab,
        onDestinationSelected: (i) => setState(() => _tab = i),
        destinations: [for (final (icon, label) in destinations) NavigationDestination(icon: icon, label: label)],
      ),
    );
  }
}

class RadarPage extends StatelessWidget {
  const RadarPage({super.key});

  void _details(BuildContext context, Device d) {
    final s = S.of(context);
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (sheetContext) {
        final theme = Theme.of(sheetContext);
        return SafeArea(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    OsAvatar(os: d.os, size: 52),
                    const SizedBox(width: 14),
                    Expanded(child: Text(d.name, style: theme.textTheme.titleLarge)),
                    if (d.trusted) Icon(Icons.verified, color: theme.colorScheme.primary),
                  ],
                ),
                const SizedBox(height: 16),
                Text(s.deviceDetails, style: theme.textTheme.titleSmall),
                const SizedBox(height: 8),
                SelectableText('${s.deviceId}: ${d.id}', style: const TextStyle(fontFamily: 'monospace')),
                if (d.addresses.isNotEmpty) Text('${s.addresses}: ${d.addresses.join(', ')}'),
                Text('${s.transportsLabel}: ${d.transports.map(transportLabel).join(', ')}'),
                if (d.rssi != null) Text('${s.signal}: ${d.rssi} dBm'),
                const SizedBox(height: 16),
                FilledButton.icon(
                  onPressed: () {
                    Navigator.pop(sheetContext);
                    openDevice(context, d);
                  },
                  icon: Icon(d.reachable ? Icons.send : Icons.wifi_tethering),
                  label: Text(d.reachable ? s.sendTo(d.name) : s.connectWifiDirect),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.of(context);
    final s = S.of(context);
    final theme = Theme.of(context);
    final wide = MediaQuery.sizeOf(context).width >= 840;
    final radar = RadarView(
      devices: c.devices,
      selfOs: c.os,
      selfName: c.deviceName,
      onTap: (d) => openDevice(context, d),
      onLongPress: (d) => _details(context, d),
      onDrop: (paths, target) => sendPathsInteractive(context, paths, target: target),
    );
    final status = Wrap(
      spacing: 8,
      runSpacing: 6,
      children: [
        Chip(
          avatar: Icon(c.bleActive ? Icons.bluetooth_connected : Icons.bluetooth_disabled, size: 16),
          label: Text(c.bleActive ? 'BLE' : (c.bleError ?? 'BLE off'), overflow: TextOverflow.ellipsis),
          visualDensity: VisualDensity.compact,
        ),
        if (c.p2pConnected)
          Chip(
            avatar: const Icon(Icons.wifi_tethering, size: 16),
            label: Text(s.wifiDirectConnected),
            visualDensity: VisualDensity.compact,
          ),
        if (c.localAddresses.isNotEmpty)
          Chip(
            avatar: const Icon(Icons.lan_outlined, size: 16),
            label: Text(c.localAddresses.first),
            visualDensity: VisualDensity.compact,
          ),
      ],
    );
    final list = c.devices.isEmpty
        ? const SizedBox.shrink()
        : wide
        ? ListView(
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
                child: Text(s.nearbyDevices, style: theme.textTheme.titleSmall),
              ),
              for (final d in c.devices)
                ListTile(
                  leading: OsAvatar(os: d.os, size: 38),
                  title: Text(d.name),
                  subtitle: Text(
                    d.reachable ? d.addresses.first : s.notReachable,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                  trailing: Icon(d.reachable ? Icons.chevron_right : Icons.wifi_tethering),
                  onTap: () => openDevice(context, d),
                  onLongPress: () => _details(context, d),
                ),
            ],
          )
        : SizedBox(
            height: 54,
            child: ListView(
              scrollDirection: Axis.horizontal,
              padding: const EdgeInsets.symmetric(horizontal: 12),
              children: [
                for (final d in c.devices)
                  Padding(
                    padding: const EdgeInsets.only(right: 8),
                    child: ActionChip(
                      avatar: Icon(osIcon(d.os), size: 16, color: osColor(d.os)),
                      label: Text(d.name),
                      onPressed: () => openDevice(context, d),
                    ),
                  ),
              ],
            ),
          );
    return Scaffold(
      appBar: AppBar(
        title: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(s.appName, style: const TextStyle(fontWeight: FontWeight.w700)),
            Text(c.deviceName, style: theme.textTheme.bodySmall?.copyWith(color: theme.colorScheme.onSurfaceVariant)),
          ],
        ),
        actions: [
          if (c.wifiDirectSupported)
            IconButton(
              tooltip: s.wifiDirect,
              icon: const Icon(Icons.wifi_tethering),
              onPressed: () => showWifiDirectSheet(context),
            ),
          IconButton(tooltip: s.refresh, icon: const Icon(Icons.refresh), onPressed: c.refresh),
        ],
      ),
      body: SafeArea(
        child: wide
            ? Row(
                children: [
                  Expanded(
                    child: Padding(
                      padding: const EdgeInsets.all(16),
                      child: Column(
                        children: [
                          status,
                          Expanded(child: radar),
                        ],
                      ),
                    ),
                  ),
                  SizedBox(
                    width: 320,
                    child: Card(margin: const EdgeInsets.all(16), child: list),
                  ),
                ],
              )
            : Column(
                children: [
                  Padding(padding: const EdgeInsets.symmetric(horizontal: 12), child: status),
                  Expanded(
                    child: Padding(padding: const EdgeInsets.all(8), child: radar),
                  ),
                  list,
                  const SizedBox(height: 8),
                ],
              ),
      ),
    );
  }
}
