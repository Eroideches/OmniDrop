import 'package:flutter/material.dart';

import '../core/controller.dart';
import '../l10n.dart';

/// Lists Wi-Fi Direct peers (Android WifiP2pManager, NetworkManager on Linux, WinRT on Windows)
/// and creates a direct link. Discovery and transfer then run over the new IP link.
Future<void> showWifiDirectSheet(BuildContext context) async {
  final c = OmniScope.read(context);
  c.wifiDirectScan();
  await showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    isScrollControlled: true,
    builder: (_) => OmniScope(controller: c, child: const _WifiDirectSheet()),
  );
}

class _WifiDirectSheet extends StatelessWidget {
  const _WifiDirectSheet();

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.of(context);
    final s = S.of(context);
    final theme = Theme.of(context);
    return SafeArea(
      child: ConstrainedBox(
        constraints: BoxConstraints(maxHeight: MediaQuery.sizeOf(context).height * 0.7),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Icon(Icons.wifi_tethering, color: theme.colorScheme.primary),
                  const SizedBox(width: 10),
                  Expanded(child: Text(s.wifiDirect, style: theme.textTheme.titleLarge)),
                  if (c.p2pScanning)
                    const SizedBox(width: 22, height: 22, child: CircularProgressIndicator(strokeWidth: 2.5))
                  else
                    IconButton(onPressed: c.wifiDirectScan, icon: const Icon(Icons.refresh), tooltip: s.refresh),
                ],
              ),
              if (c.p2pConnected)
                Card(
                  child: ListTile(
                    leading: const Icon(Icons.link),
                    title: Text(s.wifiDirectConnected),
                    trailing: TextButton(onPressed: c.wifiDirectDisconnect, child: Text(s.disconnect)),
                  ),
                ),
              if (c.p2pError != null)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 8),
                  child: Text(c.p2pError!, style: TextStyle(color: theme.colorScheme.error)),
                ),
              const SizedBox(height: 8),
              Flexible(
                child: c.p2pPeers.isEmpty
                    ? Padding(
                        padding: const EdgeInsets.all(24),
                        child: Text(c.p2pScanning ? s.wifiDirectScan : s.wifiDirectNone, textAlign: TextAlign.center),
                      )
                    : ListView(
                        shrinkWrap: true,
                        children: [
                          for (final p in c.p2pPeers)
                            ListTile(
                              leading: Icon(p.omnidropId != null ? Icons.verified_outlined : Icons.devices_other),
                              title: Text(p.name),
                              subtitle: Text([p.address, p.status].where((x) => x.isNotEmpty).join(' · ')),
                              trailing: FilledButton.tonal(
                                onPressed: () {
                                  c.wifiDirectConnect(p);
                                  ScaffoldMessenger.of(context)
                                      .showSnackBar(SnackBar(content: Text(s.wifiDirectConnecting)));
                                },
                                child: Text(s.connect),
                              ),
                            ),
                        ],
                      ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
