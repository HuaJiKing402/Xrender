#!/usr/bin/env bash
set -euo pipefail

cargo ndk -t arm64-v8a -p 26 -o platforms/android/app/src/main/jniLibs \
  build -p app-android --release
cd platforms/android && ./gradlew assembleDebug
