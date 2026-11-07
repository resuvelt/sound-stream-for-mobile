# 🔊 Sound Stream

Listen to your Windows PC's audio on your phone, right in the browser. No app to install.

A single `sound-stream.exe` sits in the system tray, captures everything your speakers play
(WASAPI loopback), and streams it over Wi-Fi to any iPhone or Android browser.

## Run

1. Download `sound-stream.exe` from the latest release (Releases).
2. Run it. There's no console window, just a speaker icon in the tray.
3. On your phone, open **`http://sound-stream.local:47823/`** (same Wi-Fi) and tap play.

The exe is fully standalone. It runs on any Windows 10/11 x64 PC with nothing to install:
no Node.js, no Visual C++ runtime.

- **SmartScreen** may warn about an unknown publisher. Click **More info → Run anyway**.
- **Windows Firewall** may ask on first launch. Allow it for private networks.

## Build from source

```bash
cargo build --release
```

This needs [Rust](https://rustup.rs) 1.85+ with the MSVC toolchain and gives you
`target\release\sound-stream.exe`.

To publish a release, push an annotated version tag (`git tag -a 2.1 && git push origin 2.1`).
GitHub Actions then builds the exe and attaches it to the release, using the tag message as the
release notes.

## Tray menu

Right-click the icon for (the UI is in Russian):

- status: listener count and the device being captured
- the phone address (click to copy) and fallback IP addresses
- open the player on this PC (also a double-click on the icon)
- start with Windows (on/off)
- the log
- restart and exit

## Notes

- Capture starts with the first listener, stops when everyone leaves, and follows changes of the default output device.
- `sound-stream.local` is announced via mDNS, so the bookmark survives DHCP IP changes. If it doesn't resolve, check that your router has client/AP isolation turned off, or use the IP address from the tray menu.
- Optional environment variables: `PORT` (default `47823`) and `MDNS_NAME` (default `sound-stream`).
- Log: `%LOCALAPPDATA%\sound-stream\server.log`.

## License

MIT
