# Für das Modul

Die finale Datenbank ist [final_fingerprints.surql](final_fingerprints.surql).

Gemessen wurden alle begehbaren Flächen mit Ausnahme des Bereichs hinter dem C-Bau.

Die Messung lief wie folgt ab: Der Grundriss wurde in ein 5x6-Raster aufgeteilt. In jeder Zelle wurde das Handy möglichst mittig platziert und anschließend entlang der Bildachsen nach oben, unten, links oder rechts ausgerichtet (also nicht zwingend nach Nord, Süd, Ost oder West). Für jede Ausrichtung wurden vier aufeinanderfolgende Messungen durchgeführt. Waren Messungen identisch (etwa wegen Scan Throttling) oder schlugen sie fehl, wurden die Daten dieser Zelle in dieser Ausrichtung verworfen und eine weitere Messung durchgeführt.

Gefiltert wurden die Netzwerke wie folgt:
- SSID `IN-foo` erlaubt
- SSID `eduroam` erlaubt
- SSIDs, die `hfu` enthalten, erlaubt (unabhängig von Groß-/Kleinschreibung)
- Daten aller übrigen Netze wurden entfernt und sind nicht im Datensatz enthalten



# WCN Fingerprinting

Rust + Tauri 2 + SurrealDB Android app for Wi-Fi fingerprint capture on the `WLAN_AP_in-EG_C-Bau.jpg` floorplan.

## What It Does

- Displays the floorplan image with a selectable 5 x 6 grid.
- Labels cells from `a0` through `e5`, with `a0` at the top-left.
- Requires one orientation per capture: `u`, `d`, `l`, or `r`.
- Captures four fresh Android Wi-Fi scans after `Start fingerprinting`.
- Stores only networks whose SSID is exactly `IN-foo`, exactly `eduroam`, or contains `hfu` case-insensitively.
- Persists each pose as `Fingerprints:<cell><orientation>`, for example `Fingerprints:a0u`.
- Exports the database through the `Save DB backup` button as a `.surql` file selected by Android's document picker.

## Persistence

The app uses embedded SurrealDB with SurrealKV in the app-private data directory. The database persists after closing the app and after phone restarts. It is removed if Android app data is cleared or the app is uninstalled.

## Local Toolchain

This workspace is configured for Android builds with:

- JDK: `$HOME/.local/share/jdks/temurin-17`
- Android SDK: `$HOME/Android/Sdk`
- Android NDK: `$HOME/Android/Sdk/ndk/29.0.14206865`

The same environment exports were appended to `~/.profile` and `~/.bashrc`.

For the current shell:

```sh
source ~/.profile
```

## Build And Run

Install dependencies:

```sh
npm install
```

Run on a connected Android phone:

```sh
npm run tauri android dev
```

Build a release APK/AAB:

```sh
npm run tauri android build
```

The latest verified build produced:

```text
src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
src-tauri/gen/android/app/build/outputs/bundle/universalRelease/app-universal-release.aab
```

The release APK is unsigned. Use `android dev` for direct debugging/install, or configure signing before distributing the release APK.

## Phone Setup

On the Android phone:

- Enable Wi-Fi.
- Enable Location services.
- Grant the app precise location permission when prompted.
- On Android 13 or newer, grant nearby Wi-Fi/devices permission when prompted.
- For repeated real-time scans, enable Developer options and disable Wi-Fi scan throttling if the phone exposes that toggle.

Android may still reject or delay repeated scans depending on device firmware, power state, and system policy. The app treats stale scan broadcasts as failures because fingerprinting needs fresh BSSID/RSSI data.
