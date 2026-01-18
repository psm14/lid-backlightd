# lid-backlightd

Made this because I repurposed an old XPS laptop as a server, and when configuring it to not sleep when the lid closed the screen would stay on. This forces the screen off when the lid is closed by listening on dbus. On my machine it sits at ~5mb of memory and mostly does nothing cpu-wise. It was also vibe-coded and I am pretty satisfied with the result given the small effort I put into this. Alright, the rest of this readme is generated:

## Build

```bash
cargo build --release
```

## Install

```bash
sudo install -m 0755 target/release/lid-backlightd /usr/bin/
sudo install -m 0644 lid-backlightd.service /etc/systemd/system/lid-backlightd.service
sudo systemctl daemon-reload
sudo systemctl enable --now lid-backlightd
```

## Debian/Ubuntu package

```bash
scripts/build-deb.sh
sudo dpkg -i dist/deb/lid-backlightd_*.deb
```

## Flags

- `--device <name>`: pin a specific backlight device (e.g. `intel_backlight`)
- `--restore-min <n>`: minimum brightness to restore (default `1`, set `0` to allow restore to 0)
- `--log-level <level>`: log filter (defaults to `RUST_LOG` or `info`)

## Notes

- The service expects `/sys/class/backlight/*/brightness` and `/sys/class/backlight/*/max_brightness`.
- If no device is found at startup, it keeps running and retries on lid events.
- Run as root for simplest permissions, or adjust udev/ACLs to allow brightness writes.

## Quick test

```bash
busctl get-property org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager LidClosed
sudo /usr/bin/lid-backlightd --log-level debug
```

## License

WTFPL. See `LICENSE`.
