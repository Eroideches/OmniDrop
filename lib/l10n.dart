import 'package:flutter/widgets.dart';

/// UI strings in Italian and English (chosen from the system locale).
class S {
  const S(this.it);

  final bool it;

  static S of(BuildContext context) {
    final locale = Localizations.maybeLocaleOf(context);
    return S(locale?.languageCode == 'it');
  }

  String _(String italian, String english) => it ? italian : english;

  String get appName => 'OmniDrop';
  String get radar => _('Radar', 'Radar');
  String get transfers => _('Trasferimenti', 'Transfers');
  String get settings => _('Impostazioni', 'Settings');
  String get you => _('Tu', 'You');
  String get searching => _('Ricerca di dispositivi vicini…', 'Looking for nearby devices…');
  String get searchingHint => _(
    'Apri OmniDrop sugli altri dispositivi. Funziona sulla stessa rete Wi-Fi, via Bluetooth e Wi-Fi Direct.',
    'Open OmniDrop on your other devices. Works on the same Wi-Fi, over Bluetooth and Wi-Fi Direct.',
  );
  String get nearbyDevices => _('Dispositivi vicini', 'Nearby devices');
  String get refresh => _('Aggiorna', 'Refresh');
  String get dropHere => _('Rilascia su un dispositivo per inviare', 'Drop on a device to send');
  String get chooseDevice => _('Scegli il dispositivo', 'Choose a device');
  String sendTo(String name) => _('Invia a $name', 'Send to $name');
  String get files => _('File', 'Files');
  String get folder => _('Cartella', 'Folder');
  String get clipboard => _('Appunti', 'Clipboard');
  String get textOrLink => _('Testo o link', 'Text or link');
  String get browseStorage => _('Sfoglia memoria', 'Browse storage');
  String get systemPicker => _('Selettore di sistema', 'System picker');
  String get send => _('Invia', 'Send');
  String get cancel => _('Annulla', 'Cancel');
  String get close => _('Chiudi', 'Close');
  String get accept => _('Accetta', 'Accept');
  String get decline => _('Rifiuta', 'Decline');
  String get pause => _('Pausa', 'Pause');
  String get resume => _('Riprendi', 'Resume');
  String get remove => _('Rimuovi', 'Remove');
  String get open => _('Apri', 'Open');
  String get openFolder => _('Apri cartella', 'Open folder');
  String get copy => _('Copia', 'Copy');
  String get copied => _('Copiato negli appunti', 'Copied to clipboard');
  String get openLink => _('Apri link', 'Open link');
  String get clipboardEmpty => _('Gli appunti sono vuoti', 'The clipboard is empty');
  String get typeTextHint => _('Scrivi un messaggio o incolla un link', 'Type a message or paste a link');
  String get notReachable =>
      _('Visto via Bluetooth, non sulla stessa rete', 'Seen over Bluetooth, not on the same network');
  String get connectWifiDirect => _('Collega con Wi-Fi Direct', 'Connect with Wi-Fi Direct');
  String get wifiDirect => 'Wi-Fi Direct';
  String get wifiDirectScan => _('Cerca dispositivi Wi-Fi Direct', 'Scan for Wi-Fi Direct devices');
  String get wifiDirectNone => _('Nessun dispositivo Wi-Fi Direct trovato', 'No Wi-Fi Direct devices found');
  String get wifiDirectConnecting => _('Connessione Wi-Fi Direct…', 'Connecting with Wi-Fi Direct…');
  String get wifiDirectConnected => _('Collegamento Wi-Fi Direct attivo', 'Wi-Fi Direct link active');
  String get disconnect => _('Disconnetti', 'Disconnect');
  String get connect => _('Connetti', 'Connect');
  String get incomingTitle => _('Richiesta in arrivo', 'Incoming request');
  String wantsToSend(String name) => _('$name vuole inviarti', '$name wants to send you');
  String filesCount(int n) => it ? (n == 1 ? '1 file' : '$n file') : (n == 1 ? '1 file' : '$n files');
  String moreFiles(int n) => _('e altri $n', 'and $n more');
  String get aMessage => _('un messaggio', 'a message');
  String get verifyCode => _('Codice di verifica', 'Verification code');
  String verifyCodeHint(String name) => _(
    'Controlla che su $name compaiano lo stesso PIN e le stesse emoji. Se sono diversi, rifiuta: qualcuno potrebbe intercettare la connessione.',
    'Check that $name shows the same PIN and emoji. If they differ, decline: someone may be intercepting the connection.',
  );
  String get trustDevice => _('Considera attendibile questo dispositivo', 'Trust this device');
  String get trustedDevice => _('Dispositivo attendibile', 'Trusted device');
  String waitingFor(String name) => _('In attesa che $name accetti…', 'Waiting for $name to accept…');
  String get compareCode => _(
    'Confronta questo codice con quello sull\'altro dispositivo',
    'Compare this code with the one on the other device',
  );
  String get messageReceived => _('Messaggio ricevuto', 'Message received');
  String from(String name) => _('Da $name', 'From $name');
  String get noTransfers => _('Nessun trasferimento', 'No transfers yet');
  String get noTransfersHint => _(
    'Tocca un dispositivo sul radar o trascina dei file su di esso.',
    'Tap a device on the radar or drag files onto it.',
  );
  String get clearFinished => _('Pulisci completati', 'Clear finished');
  String get sending => _('Invio a', 'Sending to');
  String get receiving => _('Ricezione da', 'Receiving from');
  String eta(String t) => _('Tempo stimato $t', 'ETA $t');
  String blocks(int done, int total) => _('$done/$total blocchi', '$done/$total blocks');
  String streams(int n) => _('$n flussi', '$n streams');
  String savedIn(String path) => _('Salvato in $path', 'Saved in $path');

  String state(String s) {
    switch (s) {
      case 'preparing':
        return _('Preparazione', 'Preparing');
      case 'connecting':
        return _('Connessione', 'Connecting');
      case 'waiting_accept':
        return _('In attesa di conferma', 'Waiting for confirmation');
      case 'verifying':
        return _('Verifica blocchi', 'Verifying blocks');
      case 'transferring':
        return _('In corso', 'Transferring');
      case 'finalizing':
        return _('Verifica finale', 'Final check');
      case 'reconnecting':
        return _('Riconnessione…', 'Reconnecting…');
      case 'interrupted':
        return _('Interrotto, in attesa di ripresa', 'Interrupted, waiting to resume');
      case 'completed':
        return _('Completato', 'Completed');
      case 'rejected':
        return _('Rifiutato', 'Declined');
      case 'cancelled':
        return _('Annullato', 'Cancelled');
      case 'failed':
        return _('Errore', 'Failed');
      default:
        return s;
    }
  }

  String get paused => _('In pausa', 'Paused');
  String get deviceName => _('Nome del dispositivo', 'Device name');
  String get downloadFolder => _('Cartella di ricezione', 'Download folder');
  String get change => _('Cambia', 'Change');
  String get discovery => _('Rilevamento', 'Discovery');
  String get mdnsTitle => _('mDNS / DNS-SD', 'mDNS / DNS-SD');
  String get mdnsSubtitle => _('Rilevamento standard sulla rete locale', 'Standard local network discovery');
  String get broadcastTitle => _('Broadcast UDP', 'UDP broadcast');
  String get broadcastSubtitle =>
      _('Trova dispositivi anche dove mDNS è bloccato', 'Finds devices where mDNS is blocked');
  String get bleTitle => 'Bluetooth Low Energy';
  String get bleSubtitle => _('Rileva dispositivi vicini senza rete', 'Detects nearby devices without a network');
  String get wifiDirectSubtitle => _('Collegamento diretto senza router', 'Direct link without a router');
  String get transferSection => _('Trasferimento', 'Transfer');
  String get parallelStreams => _('Flussi paralleli', 'Parallel streams');
  String get autoAccept => _('Accetta automaticamente dai dispositivi attendibili', 'Auto-accept from trusted devices');
  String get trustedDevices => _('Dispositivi attendibili', 'Trusted devices');
  String get noTrusted => _('Nessun dispositivo attendibile', 'No trusted devices');
  String get addByIp => _('Aggiungi dispositivo tramite IP', 'Add device by IP address');
  String get ipAddress => _('Indirizzo IP (es. 192.168.1.20)', 'IP address (e.g. 192.168.1.20)');
  String get add => _('Aggiungi', 'Add');
  String get thisDevice => _('Questo dispositivo', 'This device');
  String get deviceId => _('ID dispositivo', 'Device ID');
  String get fingerprint => _('Impronta della chiave', 'Key fingerprint');
  String get addresses => _('Indirizzi', 'Addresses');
  String get security => _('Sicurezza', 'Security');
  String get securityInfo => _(
    'Noise_XX (Curve25519) + AES-256-GCM con accelerazione hardware, altrimenti ChaCha20-Poly1305. Integrità SHA-256 per blocco.',
    'Noise_XX (Curve25519) + hardware-accelerated AES-256-GCM, otherwise ChaCha20-Poly1305. Per-block SHA-256 integrity.',
  );
  String get aesHw => _('AES hardware', 'Hardware AES');
  String get yes => _('Sì', 'Yes');
  String get no => _('No', 'No');
  String get ioBackend => _('I/O su disco', 'Disk I/O');
  String get permissions => _('Permessi', 'Permissions');
  String get permissionsIntro => _(
    'OmniDrop usa Bluetooth e Wi-Fi per trovare i dispositivi vicini e l\'accesso ai file per inviare e salvare i trasferimenti.',
    'OmniDrop uses Bluetooth and Wi-Fi to find nearby devices and file access to send and save transfers.',
  );
  String get permNearby => _('Dispositivi Wi-Fi e Bluetooth vicini', 'Nearby Wi-Fi and Bluetooth devices');
  String get permLocation => _(
    'Posizione (richiesta da Android per Bluetooth/Wi-Fi Direct)',
    'Location (required by Android for Bluetooth/Wi-Fi Direct)',
  );
  String get permStorage => _('Accesso a tutti i file', 'All files access');
  String get permNotifications => _('Notifiche di avanzamento', 'Progress notifications');
  String get permBattery => _('Nessuna ottimizzazione batteria', 'No battery optimisation');
  String get grant => _('Consenti', 'Allow');
  String get granted => _('Consentito', 'Allowed');
  String get continueLabel => _('Continua', 'Continue');
  String get later => _('Più tardi', 'Later');
  String get internalStorage => _('Memoria interna', 'Internal storage');
  String get selectThisFolder => _('Invia questa cartella', 'Send this folder');
  String get useThisFolder => _('Usa questa cartella', 'Use this folder');
  String selected(int n) => _('$n selezionati', '$n selected');
  String get emptyFolder => _('Cartella vuota', 'Empty folder');
  String get storageDenied =>
      _('Concedi "Accesso a tutti i file" per sfogliare la memoria.', 'Grant "All files access" to browse storage.');
  String get deviceDetails => _('Dettagli dispositivo', 'Device details');
  String get transportsLabel => _('Collegamenti', 'Links');
  String get signal => _('Segnale', 'Signal');
  String get engineError => _('Impossibile avviare il motore OmniDrop', 'Could not start the OmniDrop engine');
  String get retry => _('Riprova', 'Retry');
  String get requestExpired => _('La richiesta è scaduta', 'The request expired');
  String get incomingCancelled => _('Il mittente ha annullato l\'invio', 'The sender cancelled');
  String get firewallHint => _(
    'Su Windows consenti OmniDrop nel firewall (lo script firewall-setup.ps1 è incluso).',
    'On Windows allow OmniDrop through the firewall (firewall-setup.ps1 is included).',
  );
  String get sharedWith => _('Condividi con', 'Share with');
  String get about => _('Informazioni', 'About');
  String get license => _('Software libero con licenza MIT', 'Free software, MIT licence');
}
