#!/usr/bin/env bash
# Builds the Rust core (native/) for a target platform.
#
#   scripts/build-native.sh android   # cargo-ndk -> android/app/src/main/jniLibs/<abi>/libomnidrop_core.so
#   scripts/build-native.sh host      # cargo     -> native/target/release/ (Linux .so / Windows .dll)
#
# Linux and Windows builds do not need this script: `flutter build linux|windows` runs cargo
# through CMake. Android needs cargo-ndk (`cargo install cargo-ndk`), the Rust targets
# aarch64-linux-android, armv7-linux-androideabi, x86_64-linux-android and ANDROID_NDK_HOME.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/native"

case "${1:-}" in
  android)
    cargo ndk --platform 26 \
      -t arm64-v8a -t armeabi-v7a -t x86_64 \
      -o "$ROOT/android/app/src/main/jniLibs" \
      build --release
    ls -l "$ROOT"/android/app/src/main/jniLibs/*/libomnidrop_core.so
    ;;
  host)
    cargo build --release
    ;;
  *)
    echo "usage: $0 android|host" >&2
    exit 2
    ;;
esac
