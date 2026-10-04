import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import 'core/controller.dart';
import 'l10n.dart';
import 'ui/home_screen.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  final controller = OmniController();
  runApp(OmniDropApp(controller: controller));
  controller.init();
}

ThemeData _theme(Brightness brightness) {
  final scheme = ColorScheme.fromSeed(seedColor: const Color(0xFF5B8CFF), brightness: brightness);
  final dark = brightness == Brightness.dark;
  final base = dark
      ? scheme.copyWith(surface: const Color(0xFF0E1218), surfaceContainerLow: const Color(0xFF141922))
      : scheme;
  return ThemeData(
    useMaterial3: true,
    colorScheme: base,
    scaffoldBackgroundColor: base.surface,
    cardTheme: CardThemeData(
      elevation: 0,
      color: base.surfaceContainerLow,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(20)),
    ),
    appBarTheme: AppBarTheme(backgroundColor: base.surface, scrolledUnderElevation: 0),
    snackBarTheme: const SnackBarThemeData(behavior: SnackBarBehavior.floating),
  );
}

class OmniDropApp extends StatelessWidget {
  const OmniDropApp({super.key, required this.controller});

  final OmniController controller;

  @override
  Widget build(BuildContext context) {
    return OmniScope(
      controller: controller,
      child: MaterialApp(
        title: 'OmniDrop',
        debugShowCheckedModeBanner: false,
        theme: _theme(Brightness.light),
        darkTheme: _theme(Brightness.dark),
        themeMode: ThemeMode.system,
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        supportedLocales: const [Locale('en'), Locale('it')],
        home: const _Root(),
      ),
    );
  }
}

class _Root extends StatelessWidget {
  const _Root();

  @override
  Widget build(BuildContext context) {
    final c = OmniScope.of(context);
    final s = S.of(context);
    if (c.startError != null) {
      return Scaffold(
        body: Center(
          child: Padding(
            padding: const EdgeInsets.all(32),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(Icons.error_outline, size: 56, color: Theme.of(context).colorScheme.error),
                const SizedBox(height: 16),
                Text(s.engineError, style: Theme.of(context).textTheme.titleMedium),
                const SizedBox(height: 8),
                SelectableText(c.startError!, textAlign: TextAlign.center),
              ],
            ),
          ),
        ),
      );
    }
    if (!c.ready) {
      return Scaffold(
        body: Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Image.asset('assets/icon.png', width: 96, height: 96),
              const SizedBox(height: 24),
              const CircularProgressIndicator(),
            ],
          ),
        ),
      );
    }
    return const HomeScreen();
  }
}
