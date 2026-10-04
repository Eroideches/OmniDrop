import 'dart:io';

import 'package:flutter/material.dart';

import '../core/format.dart';
import '../l10n.dart';
import 'widgets.dart';

enum BrowserMode { send, pickFolder }

/// Direct file-system browser for Android (requires "All files access"): selects real paths, so
/// large files are streamed by the engine without being copied first.
class FileBrowser extends StatefulWidget {
  const FileBrowser({super.key, required this.root, required this.mode});

  final String root;
  final BrowserMode mode;

  @override
  State<FileBrowser> createState() => _FileBrowserState();
}

class _Entry {
  _Entry(this.path, this.isDir, this.size, this.modified);

  final String path;
  final bool isDir;
  final int size;
  final DateTime modified;
}

class _FileBrowserState extends State<FileBrowser> {
  late String _dir = widget.root;
  final Set<String> _selected = {};
  List<_Entry> _entries = const [];
  String? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final items = <_Entry>[];
      await for (final e in Directory(_dir).list(followLinks: false)) {
        final name = basename(e.path);
        if (name.startsWith('.')) continue;
        final stat = await e.stat();
        final isDir = stat.type == FileSystemEntityType.directory;
        if (widget.mode == BrowserMode.pickFolder && !isDir) continue;
        items.add(_Entry(e.path, isDir, stat.size, stat.modified));
      }
      items.sort((a, b) {
        if (a.isDir != b.isDir) return a.isDir ? -1 : 1;
        return basename(a.path).toLowerCase().compareTo(basename(b.path).toLowerCase());
      });
      setState(() {
        _entries = items;
        _error = null;
      });
    } on FileSystemException catch (e) {
      setState(() {
        _entries = const [];
        _error = e.osError?.message ?? e.message;
      });
    }
  }

  void _open(String dir) {
    setState(() => _dir = dir);
    _load();
  }

  bool get _atRoot => _dir == widget.root;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final rel = _dir.length > widget.root.length ? _dir.substring(widget.root.length) : '/';
    return PopScope(
      canPop: _atRoot,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _open(Directory(_dir).parent.path);
      },
      child: Scaffold(
        appBar: AppBar(
          title: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(s.internalStorage),
              Text(rel, style: theme.textTheme.bodySmall, overflow: TextOverflow.ellipsis),
            ],
          ),
          actions: [
            if (_selected.isNotEmpty) TextButton(onPressed: () => setState(_selected.clear), child: Text(s.cancel)),
          ],
        ),
        body: _error != null
            ? EmptyState(icon: Icons.lock_outline, title: _error!, subtitle: s.storageDenied)
            : _entries.isEmpty
            ? EmptyState(icon: Icons.folder_off_outlined, title: s.emptyFolder)
            : ListView.builder(
                itemCount: _entries.length,
                itemBuilder: (context, i) {
                  final e = _entries[i];
                  final selected = _selected.contains(e.path);
                  return ListTile(
                    leading: Icon(
                      fileKindIcon(kindOf(e.path, dir: e.isDir)),
                      color: e.isDir ? theme.colorScheme.primary : null,
                    ),
                    title: Text(basename(e.path), maxLines: 1, overflow: TextOverflow.ellipsis),
                    subtitle: e.isDir ? null : Text(formatBytes(e.size)),
                    trailing: widget.mode == BrowserMode.send
                        ? Checkbox(
                            value: selected,
                            onChanged: (v) => setState(() {
                              if (v == true) {
                                _selected.add(e.path);
                              } else {
                                _selected.remove(e.path);
                              }
                            }),
                          )
                        : null,
                    onTap: () {
                      if (e.isDir) {
                        _open(e.path);
                      } else {
                        setState(() => selected ? _selected.remove(e.path) : _selected.add(e.path));
                      }
                    },
                    onLongPress: widget.mode == BrowserMode.send
                        ? () => setState(() => selected ? _selected.remove(e.path) : _selected.add(e.path))
                        : null,
                  );
                },
              ),
        bottomNavigationBar: SafeArea(
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: widget.mode == BrowserMode.pickFolder
                ? FilledButton.icon(
                    icon: const Icon(Icons.check),
                    label: Text(s.useThisFolder),
                    onPressed: () => Navigator.pop(context, [_dir]),
                  )
                : Row(
                    children: [
                      Expanded(
                        child: OutlinedButton.icon(
                          icon: const Icon(Icons.drive_folder_upload_outlined),
                          label: Text(s.selectThisFolder, overflow: TextOverflow.ellipsis),
                          onPressed: _atRoot ? null : () => Navigator.pop(context, [_dir]),
                        ),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: FilledButton.icon(
                          icon: const Icon(Icons.send),
                          label: Text(_selected.isEmpty ? s.send : '${s.send} (${_selected.length})'),
                          onPressed: _selected.isEmpty ? null : () => Navigator.pop(context, _selected.toList()),
                        ),
                      ),
                    ],
                  ),
          ),
        ),
      ),
    );
  }
}
