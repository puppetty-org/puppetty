#!/bin/sh
set -eu

BASE_URL="${BASE_URL:-https://puppetty-org.github.io/puppetty/gui}"
INSTALL_DIR="${INSTALL_DIR:-}"
QUIET="${QUIET:-}"

say() {
  if [ -z "$QUIET" ]; then
    printf 'puppetty-gui: %s\n' "$1"
  fi
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'puppetty-gui: need %s\n' "$1" >&2
    exit 1
  fi
}

download() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$out"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$out"
  else
    printf 'puppetty-gui: need curl or wget\n' >&2
    exit 1
  fi
}

sha256_file() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    printf 'puppetty-gui: need sha256sum or shasum\n' >&2
    exit 1
  fi
}

os="$(uname -s)"
cpu="$(uname -m)"
case "$os:$cpu" in
  Linux:x86_64 | Linux:amd64)
    pkg="linux-x64"
    default_dir="${HOME}/.local/share/puppetty-gui"
    ;;
  Darwin:x86_64 | Darwin:amd64)
    pkg="darwin-x64"
    default_dir="${HOME}/Applications/puppetty-gui"
    ;;
  Darwin:arm64 | Darwin:aarch64)
    pkg="darwin-arm64"
    default_dir="${HOME}/Applications/puppetty-gui"
    ;;
  *)
    printf 'puppetty-gui: unsupported platform: %s %s\n' "$os" "$cpu" >&2
    exit 1
    ;;
esac

if [ -z "$INSTALL_DIR" ]; then
  INSTALL_DIR="$default_dir"
fi

need_cmd unzip
need_cmd awk
need_cmd sed

base="$(printf '%s' "$BASE_URL" | sed 's:/*$::')"
package="puppetty-gui-${pkg}.zip"
tmp="${TMPDIR:-/tmp}/puppetty-gui-install.$$"
mkdir -p "$tmp"

cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

package_path="${tmp}/puppetty-gui.zip"
sha_path="${tmp}/puppetty-gui.zip.sha256"

say "downloading ${base}/latest/${package}"
download "${base}/latest/${package}" "$package_path"
download "${base}/latest/${package}.sha256" "$sha_path"

actual="$(sha256_file "$package_path")"
expected="$(awk '{print $1}' "$sha_path")"
if [ "$actual" != "$expected" ]; then
  printf 'puppetty-gui: checksum mismatch for downloaded package\n' >&2
  exit 1
fi

say "installing to ${INSTALL_DIR}"
rm -rf "${tmp}/payload"
mkdir -p "${tmp}/payload" "$INSTALL_DIR"
unzip -q "$package_path" -d "${tmp}/payload"

if [ ! -f "${tmp}/payload/puppetty-gui" ]; then
  printf 'puppetty-gui: package is missing puppetty-gui\n' >&2
  exit 1
fi
if [ ! -f "${tmp}/payload/puppetty-engine" ]; then
  printf 'puppetty-gui: package is missing puppetty-engine\n' >&2
  exit 1
fi

cp -R "${tmp}/payload/." "$INSTALL_DIR/"
chmod +x "${INSTALL_DIR}/puppetty-gui" "${INSTALL_DIR}/puppetty-engine"

cat > "${INSTALL_DIR}/uninstall.sh" <<'EOF'
#!/bin/sh
set -eu
install_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
rm -f "${HOME}/.local/bin/puppetty-gui"
rm -f "${HOME}/.local/share/applications/puppetty-gui.desktop"
rm -rf "$install_dir"
EOF
chmod +x "${INSTALL_DIR}/uninstall.sh"

if [ "$os" = "Linux" ]; then
  mkdir -p "${HOME}/.local/bin" "${HOME}/.local/share/applications"
  ln -sf "${INSTALL_DIR}/puppetty-gui" "${HOME}/.local/bin/puppetty-gui"
  cat > "${HOME}/.local/share/applications/puppetty-gui.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=puppetty-gui
Exec=${INSTALL_DIR}/puppetty-gui
Terminal=false
Categories=Development;TerminalEmulator;
EOF
elif [ "$os" = "Darwin" ]; then
  mkdir -p "${HOME}/.local/bin"
  ln -sf "${INSTALL_DIR}/puppetty-gui" "${HOME}/.local/bin/puppetty-gui"
fi

say "installed"
