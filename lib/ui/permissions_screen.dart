import 'dart:io';

import 'package:flutter/material.dart';
import 'package:permission_handler/permission_handler.dart';

import '../core/controller.dart';
import '../l10n.dart';

/// Android runtime permissions: nearby devices (BLE scan/advertise/connect, Wi-Fi Direct),
/// location (required by Android <= 12 for scans), all files access, notifications and the
/// battery optimisation exemption that keeps long transfers alive.
class PermissionsScreen extends StatefulWidget {
  const PermissionsScreen({super.key, this.onboarding = false});

  final bool onboarding;

  static Future<bool> essentialsGranted() async {
    if (!Platform.isAndroid) return true;
    final nearby = await Permission.bluetoothScan.isGranted && await Permission.nearbyWifiDevices.isGranted;
    final storage = await Permission.manageExternalStorage.isGranted || await Permission.storage.isGranted;
    return nearby && storage;
  }

  @override
  State<PermissionsScreen> createState() => _PermissionsScreenState();
}

class _PermissionsScreenState extends State<PermissionsScreen> with WidgetsBindingObserver {
  final Map<String, bool> _status = {};

  static const _nearby = [
    Permission.bluetoothScan,
    Permission.bluetoothConnect,
    Permission.bluetoothAdvertise,
    Permission.nearbyWifiDevices,
  ];

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _refresh();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) _refresh();
  }

  Future<bool> _all(List<Permission> perms) async {
    for (final p in perms) {
      if (!await p.isGranted) return false;
    }
    return true;
  }

  Future<void> _refresh() async {
    final status = {
      'nearby': await _all(_nearby),
      'location': await Permission.locationWhenInUse.isGranted,
      'storage': await Permission.manageExternalStorage.isGranted || await Permission.storage.isGranted,
      'notifications': await Permission.notification.isGranted,
      'battery': await Permission.ignoreBatteryOptimizations.isGranted,
    };
    if (!mounted) return;
    setState(
      () => _status
        ..clear()
        ..addAll(status),
    );
  }

  Future<void> _request(String key) async {
    switch (key) {
      case 'nearby':
        await _nearby.request();
      case 'location':
        await Permission.locationWhenInUse.request();
      case 'storage':
        final r = await Permission.manageExternalStorage.request();
        if (!r.isGranted) await Permission.storage.request();
      case 'notifications':
        await Permission.notification.request();
      case 'battery':
        await Permission.ignoreBatteryOptimizations.request();
    }
    await _refresh();
    if (!mounted) return;
    // Radios need the new permissions: restart BLE / Wi-Fi Direct.
    await OmniScope.read(context).startAndroidRadios();
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final rows = <(String, IconData, String)>[
      ('nearby', Icons.bluetooth_searching, s.permNearby),
      ('location', Icons.location_on_outlined, s.permLocation),
      ('storage', Icons.folder_outlined, s.permStorage),
      ('notifications', Icons.notifications_outlined, s.permNotifications),
      ('battery', Icons.battery_saver_outlined, s.permBattery),
    ];
    return Scaffold(
      appBar: AppBar(title: Text(s.permissions), automaticallyImplyLeading: !widget.onboarding),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Text(s.permissionsIntro, style: theme.textTheme.bodyLarge),
          const SizedBox(height: 16),
          for (final (key, icon, label) in rows)
            Card(
              child: ListTile(
                leading: Icon(icon),
                title: Text(label),
                trailing: _status[key] == true
                    ? Chip(avatar: const Icon(Icons.check, size: 16), label: Text(s.granted))
                    : FilledButton.tonal(onPressed: () => _request(key), child: Text(s.grant)),
              ),
            ),
        ],
      ),
      bottomNavigationBar: widget.onboarding
          ? SafeArea(
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: FilledButton(
                  onPressed: () => Navigator.pop(context),
                  child: Text(_status.values.every((v) => v) ? s.continueLabel : s.later),
                ),
              ),
            )
          : null,
    );
  }
}
