# LeapFrog VR

Hand tracking for your Index controllers, from an Ultraleap / Leap Motion camera on your headset.

Built and tested with an OG Leap Motion controller attached to a Valve Index, Ultraleap Gemini 5.20 and VRChat.

## How it works

LeapFrog hooks three calls between the controller driver and SteamVR.
While a controller is put down it replaces that controller's position and its 31 finger bones with the Leap's.
Nothing connects or disconnects, which matters because SteamVR stops reloading bindings for a game after
about a hundred device changes and would force you to reboot (ask me how I know).

The switch is per hand and automatic:

- **Put down**: When the controller is still and your hand is a certain distance away from it, the Leap takes over.
- **Picked up**: When the controller moves, or your hand comes back to it, the controller takes over.
- **Occluded**: When the base stations cannot see the controller but the Leap sees your
  hand, the leap takes over as well.

LeapFrog continuously learns where your wrist sits on each controller while you hold it to find the away distance.

There's also a stand-alone mode for when your controllers are off. By default, it reports to the games as full
tracking devices, allowing games like VRC to be controlled by hand fully (though I wouldn't recommend it).
Note that here is no hand-only gesture control built in for the SteamVR menu.

The driver also starts the Ultraleap tracking service when SteamVR starts and stops it when
SteamVR exits, so the Leap has some time to cool down (that thing gets hot). A small watchdog stops it even
if SteamVR crashes.

## Installation

1. Install [Ultraleap Gemini](https://developer.leapmotion.com/tracking-software-download), (recommended to install through the Steam store),
   and check that its control panel shows your hands. Set the tracking mode to Head Mounted, and if you mounted it with a 
   USB A converter like I have you'll likely want to set it to inverted as well.
2. Close SteamVR and run the installer. It copies the driver to
   `%LOCALAPPDATA%\LeapFrog\driver` (by default), registers it with SteamVR, sets the Ultraleap service to
   start on demand and gives your account the right to start and stop it.
3. Start SteamVR. Put a controller down and put your hands in front of the tracker.

To remove it, use the uninstaller from apps or `uninstall.exe` in the installation folder.

## Settings

From the SteamVR dashboard you get the new **LeapFrog** tab.

- **Hand placement**: Where the Leap hand appears relative to your head, and its rotation. Nudge
  these until the tracked hand sits where your real one is.
- **Switching**: How far away and how still a controller has to be before it counts as put
  down, and how fast it has to move to count as picked up.
- **Driver**: The options below (needs a SteamVR restart).

The tab is a normal SteamVR overlay app: SteamVR starts it for you, and Settings >
Startup/Shutdown > Choose Startup Overlay Apps turns it off or on (the driver will work regardless). Anything you change there is
saved by SteamVR itself. If you would rather script it, `leapfrog-panel.exe set <setting>
<value>` and `get <setting>` do the same from a terminal while SteamVR runs.

All settings, as they appear in `steamvr.vrsettings` under `driver_leapfrog`:

| Setting | Default | What it does |
| --- | --- | --- |
| `passthrough` | true | The controller carries the Leap's hand while put down. Off, the separate Leap hand devices take over the hand role instead, which costs a binding reload per switch. |
| `exposeAsFullTrackingMode` | false | Tell games the Index controllers are full hand-tracking devices. The finger data is the same either way. In VRChat this switches to hand-tracking mode: pinch controls while the controller is down, but the pointer and the menu then come from the hand instead of the controller. Takes effect at the next SteamVR start. |
| `handDeviceFullTrackingMode` | true | The Leap hand devices used while a controller is off report full hand tracking. Off, they report the partial level controllers use. Next SteamVR start. |
| `handOffsetX/Y/Z` | -0.01 / 0.06 / -0.08 | Hand position offset in meters, relative to your head (negative Z is forward). Depends on where the camera sits on your headset. |
| `handInwardOffset` | 0.01 | Moves each hand toward the other, in meters, to close the gap when your hands touch. |
| `rotationOffsetX/Y/Z` | 0 | Hand rotation offset in degrees, for a camera mounted at an angle. |
| `autoPickupSwitch` | true | Automatic switching. Off, a powered controller always drives its hand and only switching it off hands over to the Leap. |
| `awayDistance` | 0.20 | How far your hand has to be from a resting controller, in meters, before the Leap takes over (0.35 until the grip has been learned). |
| `nearDistance` | 0.15 | How close your hand has to come to the controller for the controller to take over. |
| `stillSpeed` | 0.3 | A controller moving slower than this, in meters per second, counts as resting. |
| `pickupSpeed` | 0.4 | A controller moving faster than this counts as picked up. |
| `manageService` | true | Start and stop the Ultraleap tracking service with SteamVR. |
| `autoResumeTracking` | true | Clear a saved tracking pause at every start. Gemini remembers a pause across restarts, and a paused service tracks nothing without saying so anywhere. |
| `trackingPerformance` | `ultra` | Ultraleap's tracking mode: `ultra`, `low` or `default`. |
| `handOnObject` | true | Hint to the tracker that your hands may be holding something. |
| `fastHandMotion` | false | Hint to favor fast movement over precision. |
| `enable` | true | SteamVR's own switch for the driver, under Settings > Startup/Shutdown > Manage Add-ons. |

## Troubleshooting

- **Hands gone after a Gemini update or reinstall**: the installer resets the service to
  automatic start without rights for your account. Run `leapfrog-register.exe register` from the
  install folder once, then restart SteamVR.
- **Hand/controller position mismatch**: adjust Hand placement on the panel. The
  defaults fit a camera centered on the headset's faceplate.

## Building

The driver is written in Rust with a small C++ shim for OpenVR, cross-compiled for Windows from Linux or WSL.

Driver:
```
git clone --recurse-submodules https://github.com/Omnicrash/LeapFrog-VR.git
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin
sudo apt install clang lld llvm nsis
./build.sh            # builds everything into dist/leapfrog
```

Installer:
```
./build.sh --package
```

Tests:
```
cargo test --features host-stubs --target x86_64-unknown-linux-gnu
```

LeapC is linked from your Gemini install; point `LEAPSDK_LIB_PATH` at its `LeapSDK/lib/x64` if
it is not the Steam copy.

The workspace has four parts: the driver (`src/` and `shim/`), the
settings panel (`panel/`, Dear ImGui on Direct3D 11), the service watchdog (`watchdog/`) and the
installer with the NSIS script (`setup/`).

## Credits

The Leap-to-SteamVR math and the status icons come from [Leapify](https://github.com/Nyabsi/driver_leapify)
by Nyabsi (MIT, see `resources/LICENSE-leapify.txt`), and the hooking mechanism follows its
example. The panel draws with [Dear ImGui](https://github.com/ocornut/imgui), the installer is
made with [NSIS](https://nsis.sourceforge.io). LeapFrog is MIT licensed.
