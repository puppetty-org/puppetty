#!/bin/sh
set -eu

REPO="${REPO:-puppetty-org/puppetty}"
INSTALL_DIR="${INSTALL_DIR:-}"
QUIET="${QUIET:-}"
# Prereleases are never installed unless requested: `curl ... | CHANNEL=beta sh`.
# TAG pins an exact release (e.g. TAG=gui-v0.2.0-beta.1) and skips
# channel resolution.
CHANNEL="${CHANNEL:-latest}"
TAG="${TAG:-}"

case "$CHANNEL" in
  latest | beta) ;;
  *)
    printf 'puppetty-gui: unknown channel: %s (use latest or beta)\n' "$CHANNEL" >&2
    exit 1
    ;;
esac

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
    archive_ext="tar.gz"
    default_dir="${HOME}/.local/share/puppetty-gui"
    ;;
  Darwin:arm64)
    pkg="darwin-arm64"
    archive_ext="zip"
    default_dir="${HOME}/Applications"
    ;;
  Darwin:*)
    printf 'puppetty-gui: Intel Macs are not supported (Apple Silicon only)\n' >&2
    exit 1
    ;;
  *)
    printf 'puppetty-gui: unsupported platform: %s %s\n' "$os" "$cpu" >&2
    exit 1
    ;;
esac

if [ "$os" = "Linux" ]; then
  ldconfig_bin="$(command -v ldconfig || true)"
  if [ -z "$ldconfig_bin" ] && [ -x /sbin/ldconfig ]; then
    ldconfig_bin=/sbin/ldconfig
  fi
  if [ -n "$ldconfig_bin" ] && ! "$ldconfig_bin" -p 2>/dev/null | grep -q 'libwebkit2gtk-4\.1\.so'; then
    printf 'puppetty-gui: warning: libwebkit2gtk-4.1 was not found; the app will not start without it\n' >&2
    printf 'puppetty-gui: install it first (Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-0)\n' >&2
  fi
fi

if [ -z "$INSTALL_DIR" ]; then
  INSTALL_DIR="$default_dir"
fi

if [ "$os" = "Linux" ]; then
  need_cmd tar
fi
need_cmd awk

package="puppetty-gui-${pkg}.${archive_ext}"
tmp="${TMPDIR:-/tmp}/puppetty-gui-install.$$"
mkdir -p "$tmp"

cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

package_path="${tmp}/${package}"
sha_path="${tmp}/${package}.sha256"

# Resolve the release via the GitHub API: newest published gui-v* release
# on the requested channel that actually carries this platform's package
# (skips historical releases with other asset formats). Drafts are never
# visible to the unauthenticated API.
if [ -z "$TAG" ]; then
  say "resolving the newest ${CHANNEL} release"
  want_prerelease="false"
  if [ "$CHANNEL" = "beta" ]; then
    want_prerelease="true"
  fi
  download "https://api.github.com/repos/${REPO}/releases?per_page=30" "${tmp}/releases.json"
  TAG="$(awk -v want="$want_prerelease" -v pkg="\"${package}\"" '
    /^[[:space:]]*"tag_name":/  { gsub(/[",]/, "", $2); tag = $2 }
    /^[[:space:]]*"prerelease":/ { gsub(/,/, "", $2); pre = $2 }
    index($0, pkg) && tag ~ /^gui-v/ && pre == want { print tag; exit }
  ' "${tmp}/releases.json")"
  if [ -z "$TAG" ]; then
    printf 'puppetty-gui: no %s release with %s found in %s\n' "$CHANNEL" "$package" "$REPO" >&2
    exit 1
  fi
fi

download_base="https://github.com/${REPO}/releases/download/${TAG}"
say "downloading ${download_base}/${package}"
download "${download_base}/${package}" "$package_path"
download "${download_base}/${package}.sha256" "$sha_path"

actual="$(sha256_file "$package_path")"
expected="$(awk '{print $1}' "$sha_path")"
if [ "$actual" != "$expected" ]; then
  printf 'puppetty-gui: checksum mismatch for downloaded package\n' >&2
  exit 1
fi

say "installing to ${INSTALL_DIR}"
rm -rf "${tmp}/payload"
mkdir -p "${tmp}/payload"
if [ "$os" = "Darwin" ]; then
  # ditto preserves the executable bits and code-signature structure that
  # a generic unzip may not restore faithfully.
  ditto -x -k "$package_path" "${tmp}/payload"
else
  tar -xzf "$package_path" -C "${tmp}/payload"
fi

if [ "$os" = "Darwin" ]; then
  app="puppetty-gui.app"
  if [ ! -f "${tmp}/payload/${app}/Contents/MacOS/puppetty-gui" ]; then
    printf 'puppetty-gui: package is missing %s\n' "$app" >&2
    exit 1
  fi
  if [ ! -f "${tmp}/payload/${app}/Contents/MacOS/puppetty-engine" ]; then
    printf 'puppetty-gui: package is missing the puppetty-engine sidecar\n' >&2
    exit 1
  fi

  # Install the bundle only — INSTALL_DIR (default ~/Applications) holds
  # other apps, so never wipe the directory itself.
  mkdir -p "$INSTALL_DIR" "${HOME}/.local/bin"
  rm -rf "${INSTALL_DIR:?}/${app}"
  ditto "${tmp}/payload/${app}" "${INSTALL_DIR}/${app}"
  ln -sf "${INSTALL_DIR}/${app}/Contents/MacOS/puppetty-gui" "${HOME}/.local/bin/puppetty-gui"

  say "installed ${INSTALL_DIR}/${app}"
  say "uninstall: move ${INSTALL_DIR}/${app} to the Trash and remove ~/.local/bin/puppetty-gui"
else
  if [ ! -f "${tmp}/payload/puppetty-gui" ]; then
    printf 'puppetty-gui: package is missing puppetty-gui\n' >&2
    exit 1
  fi
  if [ ! -f "${tmp}/payload/puppetty-engine" ]; then
    printf 'puppetty-gui: package is missing puppetty-engine\n' >&2
    exit 1
  fi

  if [ -d "$INSTALL_DIR" ] && [ -n "$(ls -A "$INSTALL_DIR" 2>/dev/null)" ] \
    && [ ! -f "${INSTALL_DIR}/puppetty-gui" ]; then
    printf 'puppetty-gui: %s is not empty and does not look like a previous puppetty-gui install\n' "$INSTALL_DIR" >&2
    exit 1
  fi
  rm -rf "$INSTALL_DIR"
  mkdir -p "$INSTALL_DIR"
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

  say "installed"
fi
