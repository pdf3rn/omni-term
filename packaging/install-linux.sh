#!/bin/sh
# Instala omni-term para el usuario actual (sin permisos de administrador).
# Uso: install.sh [ruta/al/binario] [prefijo]      (prefijo por defecto: ~/.local)
#      install.sh --uninstall [prefijo]
#
# Funciona igual desde el archivo de la release (binario y `packaging/` junto al script) y desde el repositorio
# (binario en target/release, recursos en packaging/).
set -eu

here=$(cd "$(dirname "$0")" && pwd)

# Recursos: `packaging/` junto al script (archivo de la release) o el propio directorio del script (repositorio).
if [ -d "$here/packaging/icons" ]; then
    assets="$here/packaging"
else
    assets="$here"
fi

if [ "${1:-}" = "--uninstall" ]; then
    prefix=${2:-"$HOME/.local"}
    rm -f "$prefix/bin/omni-term" "$prefix/share/applications/omni-term.desktop"
    rm -f "$prefix"/share/icons/hicolor/*/apps/omni-term.png
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
    echo "omni-term desinstalado de $prefix"
    exit 0
fi

prefix=${2:-"$HOME/.local"}
if [ -n "${1:-}" ]; then
    binary=$1
elif [ -x "$here/omni-term" ]; then
    binary="$here/omni-term"
else
    binary="$here/../target/release/omni-term"
fi

[ -x "$binary" ] || { echo "No se encuentra el binario: $binary (compila con cargo build --release)" >&2; exit 1; }
[ -f "$assets/omni-term.desktop" ] || { echo "Falta $assets/omni-term.desktop: no muevas install.sh fuera de la carpeta descomprimida" >&2; exit 1; }

install -Dm755 "$binary" "$prefix/bin/omni-term"
for icon in "$assets"/icons/*/omni-term.png; do
    [ -f "$icon" ] || continue
    size=$(basename "$(dirname "$icon")")
    install -Dm644 "$icon" "$prefix/share/icons/hicolor/$size/apps/omni-term.png"
done
install -Dm644 "$assets/omni-term.desktop" "$prefix/share/applications/omni-term.desktop"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true

echo "omni-term instalado en $prefix (binario en $prefix/bin/omni-term)"
case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "Aviso: $prefix/bin no está en tu PATH; añádelo para ejecutar «omni-term» desde la terminal." ;;
esac
