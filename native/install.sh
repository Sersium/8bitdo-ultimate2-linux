#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
prefix=${PREFIX:-"$HOME/.local"}
test -x "$root/native/target/release/ultimate2-native" || {
    echo 'Build first: cargo build --release --locked --manifest-path native/Cargo.toml' >&2
    exit 1
}
install -Dm755 "$root/native/target/release/ultimate2-native" "$prefix/bin/ultimate2-native"
install -Dm644 "$root/packaging/ultimate2.svg" "$prefix/share/icons/hicolor/scalable/apps/ultimate2.svg"
mkdir -p "$prefix/share/applications"
cat > "$prefix/share/applications/io.github.sersium.Ultimate2.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=8BitDo Ultimate 2 (Native)
Comment=Controller settings, motion, lighting and firmware
Exec="$prefix/bin/ultimate2-native"
Icon=ultimate2
Terminal=false
Categories=Settings;HardwareSettings;Game;
StartupWMClass=io.github.sersium.Ultimate2
EOF
echo "Installed $prefix/bin/ultimate2-native"
