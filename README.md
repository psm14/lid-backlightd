# lid-backlightd

Simple daemon to dim the internal backlight on lid close using logind DBus signals.

## Build

```bash
cargo build --release
```

## Install

```bash
sudo install -m 0755 target/release/lid-backlightd /usr/local/bin/
sudo install -m 0644 lid-backlightd.service /etc/systemd/system/lid-backlightd.service
sudo systemctl daemon-reload
sudo systemctl enable --now lid-backlightd
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
sudo /usr/local/bin/lid-backlightd --log-level debug
```
