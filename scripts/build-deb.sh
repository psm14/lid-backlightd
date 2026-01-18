#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

if ! command -v dpkg-deb >/dev/null 2>&1; then
  echo "dpkg-deb not found; install dpkg-dev or dpkg." >&2
  exit 1
fi

cargo build --release

pkg_name="$(awk -F ' = ' '
  $0 ~ /^\[package\]/ { in_pkg=1; next }
  $0 ~ /^\[/ { in_pkg=0 }
  in_pkg && $1 ~ /^name$/ { gsub(/"/, "", $2); print $2; exit }
' Cargo.toml)"
pkg_version="$(awk -F ' = ' '
  $0 ~ /^\[package\]/ { in_pkg=1; next }
  $0 ~ /^\[/ { in_pkg=0 }
  in_pkg && $1 ~ /^version$/ { gsub(/"/, "", $2); print $2; exit }
' Cargo.toml)"
arch="$(dpkg --print-architecture)"

build_root="$project_root/dist/deb"
pkg_dir="$build_root/${pkg_name}_${pkg_version}_${arch}"
out_deb="$build_root/${pkg_name}_${pkg_version}_${arch}.deb"

rm -rf "$pkg_dir"
install -d "$pkg_dir/DEBIAN" \
  "$pkg_dir/usr/bin" \
  "$pkg_dir/lib/systemd/system"

install -m 0755 "target/release/${pkg_name}" "$pkg_dir/usr/bin/${pkg_name}"
install -m 0644 "lid-backlightd.service" "$pkg_dir/lib/systemd/system/lid-backlightd.service"

maintainer="${DEB_MAINTAINER:-lid-backlightd <noreply@localhost>}"
depends="${DEB_DEPENDS:-systemd, dbus}"
description_short="${DEB_DESCRIPTION_SHORT:-Dim internal backlight on lid close (logind dbus)}"
description_long="${DEB_DESCRIPTION_LONG:-Small daemon that dims backlight when the laptop lid closes.}"

cat > "$pkg_dir/DEBIAN/control" <<EOF
Package: $pkg_name
Version: $pkg_version
Section: utils
Priority: optional
Architecture: $arch
Maintainer: $maintainer
Depends: $depends
Description: $description_short
 $description_long
EOF

cat > "$pkg_dir/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
fi
EOF
chmod 0755 "$pkg_dir/DEBIAN/postinst"

cat > "$pkg_dir/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
fi
EOF
chmod 0755 "$pkg_dir/DEBIAN/postrm"

dpkg-deb --build --root-owner-group "$pkg_dir" "$out_deb"
echo "Wrote $out_deb"
