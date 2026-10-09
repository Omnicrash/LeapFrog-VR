#!/usr/bin/env bash
# Cross-compiles the driver from WSL (cargo-xwin, clang-cl, lld-link) and assembles the SteamVR
# driver folder in dist/leapfrog. Installing is a copy of that folder with SteamVR closed.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

install=0
package=0
case "${1:-}" in
    --install) install=1; shift ;;
    --package) package=1; shift ;;
esac

cargo xwin build --release --workspace "$@"
# The hand models are checked in as results; their generator is local and untracked.
if [ -f handmodel/Cargo.toml ]; then
    cargo run -q --manifest-path handmodel/Cargo.toml --target x86_64-unknown-linux-gnu -- resources/rendermodels
fi

out=dist/leapfrog
rm -rf "$out"
mkdir -p "$out/bin/win64"
cp driver.vrdrivermanifest leapfrog-panel.vrmanifest "$out/"
cp -r resources "$out/resources"
cp target/x86_64-pc-windows-msvc/release/driver_leapfrog.dll "$out/bin/win64/"
cp target/x86_64-pc-windows-msvc/release/leapfrog-watchdog.exe "$out/bin/win64/"
cp target/x86_64-pc-windows-msvc/release/leapfrog-panel.exe "$out/bin/win64/"
cp target/x86_64-pc-windows-msvc/release/leapfrog-register.exe "$out/"
version=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
echo "$version" > "$out/VERSION"
cp target/x86_64-pc-windows-msvc/release/driver_leapfrog.pdb "$out/bin/win64/" 2>/dev/null || true
leapsdk="${LEAPSDK_LIB_PATH:-/mnt/e/Games/Steam/steamapps/common/Ultraleap Gemini/LeapSDK/lib/x64}"
cp "$leapsdk/LeapC.dll" "$out/bin/win64/"
echo "built $out (version $version)"

# --package: the wizard installer (NSIS, setup/leapfrog.nsi) from the dist folder.
if [ "$package" = 1 ]; then
    rm -f dist/LeapFrog-*.zip
    makensis -V2 -DVERSION="$version" -DSRC="$PWD/setup" -DDIST="$PWD/dist/leapfrog" -DOUT="$PWD/dist/LeapFrog-$version-setup.exe" setup/leapfrog.nsi && echo "packaged dist/LeapFrog-$version-setup.exe"
fi

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
