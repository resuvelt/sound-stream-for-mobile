# 🔊 Sound Stream

Listen to your Windows PC's audio on your phone, right in the browser. No app to install.

A single `sound-stream.exe` captures everything your speakers play (WASAPI loopback) and
streams it over Wi-Fi to any iPhone or Android browser. It has a small window with a QR code
for your phone and lives in the system tray.

## Run

1. Download `sound-stream.exe` from the latest release (Releases).
2. Run it. A window opens with a QR code and the phone address.
3. On your phone (same Wi-Fi), scan the QR code or open **`http://sound-stream.local:47823/`**, then tap play.

It works on Windows 10/11 x64 with nothing to install.

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

## Window and tray

The interface is in Russian and follows the Windows light or dark theme.

- **Window:** a QR code for the phone, the permanent address and fallback IP addresses (click
  one to show it in the QR code, or copy it), the captured device and listener count, start with
  Windows, the log, restart and exit. Closing the window keeps the server running in the tray.
- **Tray icon:** a click opens a compact panel with the status, addresses, the player on this
  PC, start with Windows, the log, restart and exit. A double-click opens the window.
- Started with Windows, it shows only the tray icon. Running the exe again opens the window.

## Notes

- Capture starts with the first listener, stops when everyone leaves, and follows changes of the default output device.
- `sound-stream.local` is announced via mDNS, so the bookmark survives DHCP IP changes. If it doesn't resolve, check that your router has client/AP isolation turned off, or use the IP address from the window.
- Optional environment variables: `PORT` (default `47823`) and `MDNS_NAME` (default `sound-stream`).
- Log: `%LOCALAPPDATA%\sound-stream\server.log`.

## License

MIT
