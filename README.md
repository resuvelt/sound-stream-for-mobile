# 🔊 Sound Stream

Listen to your Windows PC's audio on your phone, right in the browser. No app to install.

A single `sound-stream.exe` sits in the system tray, captures everything your speakers play
(WASAPI loopback), and streams it over Wi-Fi to any iPhone or Android browser.

## Run

1. Build it once with [Rust](https://rustup.rs) 1.85+ (MSVC toolchain):

   ```bash
   cargo build --release
   ```

2. Run `target\release\sound-stream.exe`. There's no console window, just a speaker icon in the tray.
3. On your phone, open **`http://sound-stream.local:47823/`** (same Wi-Fi) and tap play.

The exe is fully standalone: the player page and the C runtime are built in. Copy it to any
Windows 10/11 x64 PC and run it, with nothing else to install. On first launch, allow it through
Windows Firewall for private networks.

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
