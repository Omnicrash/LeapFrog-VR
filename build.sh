#!/usr/bin/env bash
# Cross-compiles the driver from WSL (cargo-xwin, clang-cl, lld-link) and assembles the SteamVR
# driver folder in dist/leapfrog. Installing is a copy of that folder with SteamVR closed.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

cargo xwin build --release "$@"

out=dist/leapfrog
rm -rf "$out"
mkdir -p "$out/bin/win64"
cp driver.vrdrivermanifest "$out/"
cp -r resources "$out/resources"
cp target/x86_64-pc-windows-msvc/release/driver_leapfrog.dll "$out/bin/win64/"
cp target/x86_64-pc-windows-msvc/release/driver_leapfrog.pdb "$out/bin/win64/" 2>/dev/null || true
leapsdk="${LEAPSDK_LIB_PATH:-/mnt/e/Games/Steam/steamapps/common/Ultraleap Gemini/LeapSDK/lib/x64}"
cp "$leapsdk/LeapC.dll" "$out/bin/win64/"
echo "built $out"
