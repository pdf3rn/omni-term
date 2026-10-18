#!/bin/sh
# Instala omni-term para el usuario actual (sin permisos de administrador).
# Uso: packaging/install-linux.sh [ruta/al/binario] [prefijo]
set -eu

here=$(cd "$(dirname "$0")" && pwd)
binary=${1:-"$here/../target/release/omni-term"}
prefix=${2:-"$HOME/.local"}

[ -x "$binary" ] || { echo "No se encuentra el binario: $binary (compila con cargo build --release)" >&2; exit 1; }

install -Dm755 "$binary" "$prefix/bin/omni-term"
for icon in "$here"/icons/*/omni-term.png; do
    size=$(basename "$(dirname "$icon")")
    install -Dm644 "$icon" "$prefix/share/icons/hicolor/$size/apps/omni-term.png"
done
install -Dm644 "$here/omni-term.desktop" "$prefix/share/applications/omni-term.desktop"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
echo "omni-term instalado en $prefix (binario en $prefix/bin/omni-term)"
