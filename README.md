# Mini System Monitor

Mini System Monitor is a local-first macOS desktop widget for keeping an eye on CPU, memory, and battery. Use its menu bar icon to show or hide the widget, open Settings, or browse recorded performance in a separate History window.

- Choose a compact or expanded layout, show or hide individual cards and graphs, and use light, dark, or system appearance.
- Optionally record performance to review trends and incidents such as CPU spikes or sustained high memory use.
- Set thresholds for CPU, memory, and battery alerts. Monitoring continues while the widget is hidden.

## Download and install

The DMG is built for **Apple silicon Macs**. The disk image and bundled signature were checked on macOS 26.6. The minimum supported macOS version is still being validated.

1. On the [GitHub Releases page](https://github.com/clarinaong/mini-system-monitor/releases), look for `mini-system-monitor_0.1.0_aarch64.dmg` under **Assets**. If the DMG is unavailable, use the source instructions below.
2. Open the DMG, drag `mini-system-monitor.app` into **Applications**, and eject the disk image.
3. Open the app from Applications. Look for the waveform icon in the macOS menu bar. If the widget is hidden, choose **Show Widget**.

The preview app uses an ad-hoc signature and has no Apple notarization. Gatekeeper may block it on first launch. If you trust the download, macOS may offer **Open Anyway** in **System Settings → Privacy & Security**. Managed Macs may restrict that option. [Apple’s first-launch guidance](https://support.apple.com/en-us/102445)

## Notifications

In **Settings**, choose **Request notification permission** and allow notifications in macOS. **Send test notification** sends a silent test banner. To receive automatic alerts, enable alerts and the rules you want, then click **Apply**. **Notification sound** controls sound for automatic alerts. macOS Focus and notification settings can suppress banners and sounds. You can change permission later under **System Settings → Notifications**. Test delivery using the packaged macOS app.

## Privacy and local storage

The app keeps performance data on your Mac and works offline. History recording is off by default. If you enable **Record performance history** and click **Apply**, the app saves CPU, memory, and available battery readings at most once every five seconds, even while the widget is hidden. History older than seven days is pruned at startup and periodically while the app runs.

The app stores preferences in `settings.json`, saved widget position in `.window-state.json`, and recorded readings and incidents in `history.sqlite3` under `~/Library/Application Support/com.minisystemmonitor/`.

Turning off recording keeps existing history until it expires or you clear it. To remove it immediately, open **Settings → Performance history → Clear history…** and confirm. If recording stays enabled, new readings will be saved.

## Uninstall

1. Choose **Quit Mini System Monitor** from the menu bar.
2. Move `mini-system-monitor.app` from Applications to the Trash.
3. To remove saved settings and history too, open Finder, choose **Go → Go to Folder**, enter `~/Library/Application Support/com.minisystemmonitor/`, and move that folder to the Trash if it exists.

## Built with

- **Tauri 2 and Rust** for the native windows, menu bar, notifications, and background monitoring. One Rust worker samples CPU, memory, and battery and shares the latest readings with the UI.
- **React, TypeScript, and Vite** for the widget, Settings, and History windows.
- **SQLite via rusqlite** for opt-in history. Queries summarize readings and compare periods when enough data is available.

## Run from source

On macOS, install the Xcode Command Line Tools, stable Rust, and Node.js 22.12 or newer on a supported release line with npm. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

From the repository root:

```sh
npm ci
npm run tauri -- dev
```

To build a local ad-hoc-signed app bundle for testing:

```sh
npm run build:notification-test -- --locked
```

The app is generated under `src-tauri/target/release/bundle/macos/`. Some toolchain combinations may need an SDK override. The package was built with `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` on the validation Mac.

### Build the Apple silicon DMG

With Python 3.10 or newer, create a virtual environment outside the repo and install the packaging tools.

```sh
python3 -m venv "$HOME/.venvs/mini-system-monitor-dmg"
source "$HOME/.venvs/mini-system-monitor-dmg/bin/activate"
python -m pip install -r scripts/dmg-requirements.txt
npm run build:dmg
```

The installer is written to `src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/`. On macOS, the standard Tauri build produces the app bundle. Use `build:dmg` for the drag-to-install layout. It uses local ad-hoc signing for the app and requires no Developer ID, notarization, or Finder automation.

### Tests

```sh
npm run test:monitor
npm run test:ui
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
```
