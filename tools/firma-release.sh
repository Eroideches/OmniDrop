#!/usr/bin/env bash
# Firma gli APK release prodotti dalla CI (che escono NON firmati: su GitHub non c'è alcun segreto).
#
#   tools/firma-release.sh <apk-non-firmato>... [-o <cartella-di-uscita>]
#   tools/firma-release.sh --crea-chiave        # una sola volta: keystore + password nel portachiavi
#
# "omnidrop-android-arm64-unsigned.apk" diventa "omnidrop-android-arm64.apk".
# La password del keystore sta nel portachiavi di GNOME (libsecret) e arriva ad apksigner via stdin:
# non finisce mai su disco, nella riga di comando o nella cronologia della shell.
set -euo pipefail

PROJECT="OmniDrop"
KEYSTORE="${SIGN_KEYSTORE:-$HOME/OmniDrop-firma/omnidrop-release.jks}"
ALIAS="omnidrop"
SDK="${ANDROID_HOME:-$HOME/android-sdk}"
BUILD_TOOLS="$SDK/build-tools/$(ls "$SDK/build-tools" | sort -V | tail -1)"

keyring() { # keyring lookup|generate
  python3 - "$1" "$PROJECT" <<'PY'
import secrets, string, sys, gi
gi.require_version('Secret', '1')
from gi.repository import Secret
schema = Secret.Schema.new('org.eroideches.signing', Secret.SchemaFlags.NONE,
                           {'project': Secret.SchemaAttributeType.STRING})
attrs = {'project': sys.argv[2]}
if sys.argv[1] == 'generate':
    if Secret.password_lookup_sync(schema, attrs, None) is not None:
        sys.exit('Esiste già una password per questo progetto nel portachiavi.')
    alphabet = string.ascii_letters + string.digits
    pw = ''.join(secrets.choice(alphabet) for _ in range(32))
    Secret.password_store_sync(schema, attrs, Secret.COLLECTION_DEFAULT,
                               f'{sys.argv[2]} – keystore di firma', pw, None)
    sys.stdout.write(pw)
else:
    pw = Secret.password_lookup_sync(schema, attrs, None)
    if pw is None:
        sys.exit('Password non trovata nel portachiavi: esegui  tools/firma-release.sh --crea-chiave')
    sys.stdout.write(pw + '\n')
PY
}

if [[ "${1:-}" == "--crea-chiave" ]]; then
  [[ -e "$KEYSTORE" ]] && { echo "Il keystore esiste già: $KEYSTORE" >&2; exit 1; }
  mkdir -p "$(dirname "$KEYSTORE")"
  chmod 700 "$(dirname "$KEYSTORE")"
  # La password generata passa a keytool solo tramite l'ambiente del processo figlio.
  KS_PASS="$(keyring generate)" keytool -genkeypair -v \
    -keystore "$KEYSTORE" -storetype PKCS12 -alias "$ALIAS" \
    -keyalg RSA -keysize 4096 -validity 10000 \
    -storepass:env KS_PASS -keypass:env KS_PASS \
    -dname "CN=OmniDrop, O=Eroideches, C=IT" >/dev/null
  chmod 600 "$KEYSTORE"
  echo "Keystore creato: $KEYSTORE (password nel portachiavi, progetto $PROJECT)"
  exit 0
fi

OUT_DIR=""
APKS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) OUT_DIR="$2"; shift 2 ;;
    *) APKS+=("$1"); shift ;;
  esac
done
[[ ${#APKS[@]} -gt 0 ]] || { echo "Uso: $0 <apk-non-firmato>... [-o cartella]" >&2; exit 2; }

for IN in "${APKS[@]}"; do
  base="$(basename "$IN")"
  name="${base%-unsigned.apk}"
  [[ "$name" == "$base" ]] && name="${base%.apk}-signed"
  OUT="${OUT_DIR:-$(dirname "$IN")}/$name.apk"
  # AGP allinea già le .so a 16 KiB (requisito Android 15+): non ri-allineare se è già a posto.
  SRC="$IN"
  if ! "$BUILD_TOOLS/zipalign" -c -p 4 "$IN" >/dev/null 2>&1; then
    SRC="$(mktemp --suffix=.apk)"
    "$BUILD_TOOLS/zipalign" -p -f 4 "$IN" "$SRC"
  fi
  keyring lookup | "$BUILD_TOOLS/apksigner" sign \
    --ks "$KEYSTORE" --ks-key-alias "$ALIAS" --ks-pass stdin \
    --out "$OUT" "$SRC"
  [[ "$SRC" != "$IN" ]] && rm -f "$SRC"
  rm -f "$OUT.idsig"
  "$BUILD_TOOLS/apksigner" verify "$OUT"
  echo "Firmato: $OUT"
done
"$BUILD_TOOLS/apksigner" verify --print-certs "$OUT" | grep -E "SHA-256" | head -1
