#!/usr/bin/env bash
# Cross-compiles the driver from WSL (cargo-xwin, clang-cl, lld-link) and assembles the SteamVR
# driver folder in dist/leapfrog. Installing is a copy of that folder with SteamVR closed.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

install=0
if [ "${1:-}" = "--install" ]; then install=1; shift; fi

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

# --install: copy over the registered driver folder; refused while SteamVR holds the DLL.
if [ "$install" = 1 ]; then
    target="${LEAPFROG_INSTALL:-/mnt/e/Games/VR/leapfrog}"
    if tasklist.exe 2>/dev/null | grep -qi '^vrserver.exe'; then
        echo "SteamVR is running: close it first" >&2
        exit 1
    fi
    rm -rf "$target" && cp -r "$out" "$target"
    win=$(wslpath -w "$target")
    if ! "/mnt/e/Games/Steam/steamapps/common/SteamVR/bin/win64/vrpathreg.exe" show 2>/dev/null | tr -d '\r' | grep -q "leapfrog : $win"; then
        "/mnt/e/Games/Steam/steamapps/common/SteamVR/bin/win64/vrpathreg.exe" adddriver "$win"
    fi
    echo "installed to $win"
fi
