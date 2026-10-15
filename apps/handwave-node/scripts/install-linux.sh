#!/bin/sh
# Build the checksum-pinned source bundle on this Linux host.
set -eu

usage() {
    cat <<'EOF'
Usage: ./install-linux.sh --source-url https://HOST/source.tar.gz --sha256 HEX64 [--prefix DIR]

Builds HandWave locally with cargo build --release --locked. Requires Linux
x86_64/aarch64, Cargo/Rust, a C compiler, pkg-config, git, tar, sha256sum and
curl or wget. No sudo, dependency installation, service or connection is started.
Default installation directory: $HOME/.local/share/handwave-node
Existing node.conf and device state are preserved. Keep the URL quoted.
EOF
}
fail() { printf '%s\n' "HandWave: $*" >&2; exit 1; }
source_url= checksum= prefix=${HOME:?HOME is required}/.local/share/handwave-node
while [ "$#" -gt 0 ]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --source-url|--sha256|--prefix)
            [ "$#" -ge 2 ] || fail "Missing value for $1 (see --help)."
            case "$1" in
                --source-url) source_url=$2 ;;
                --sha256) checksum=$2 ;;
                --prefix) prefix=$2 ;;
            esac
            shift 2 ;;
        *) fail "Unknown argument: $1 (see --help)." ;;
    esac
done
case "$source_url" in https://?*) ;; *) fail "--source-url must be an HTTPS URL." ;; esac
case "$source_url" in *[[:space:][:cntrl:]]*|*'\'*) fail "Invalid source URL." ;; esac
[ "${#checksum}" -eq 64 ] || fail "--sha256 must contain 64 hexadecimal characters."
case "$checksum" in *[!0-9a-fA-F]*) fail "--sha256 must contain 64 hexadecimal characters." ;; esac
case "$prefix" in /*) ;; *) fail "--prefix must be an absolute directory." ;; esac
case "$prefix" in /|*'
'*) fail "Invalid installation directory." ;; esac
# A destination directory (including a symlink to one) changes mv semantics.
# Refuse every existing destination except an ordinary, non-symlink file.
check_destination() {
    if [ -L "$prefix/handwave-node" ] || \
        { [ -e "$prefix/handwave-node" ] && [ ! -f "$prefix/handwave-node" ]; }; then
        fail 'Installation target must be an ordinary file, not a directory or symlink.'
    fi
}
check_destination
[ "$(uname -s)" = Linux ] || fail "Only Linux is supported by this installer."
case "$(uname -m)" in x86_64|aarch64|arm64) ;; *) fail "Unsupported Linux architecture (supported: x86_64, aarch64)." ;; esac

missing=
for tool in tar sha256sum cc pkg-config cargo git awk tr mktemp cp chmod mv mkdir rm; do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
downloader=
if command -v curl >/dev/null 2>&1; then downloader=curl
elif command -v wget >/dev/null 2>&1; then downloader=wget
else missing="$missing curl-or-wget"; fi
if [ -n "$missing" ]; then
    printf '%s\n' "Missing build tools:$missing" >&2
    cat >&2 <<'EOF'
Install build prerequisites with your distribution's package manager:
  Debian/Ubuntu: apt install build-essential pkg-config git curl ca-certificates
  Fedora/RHEL:  dnf install gcc make pkgconf-pkg-config git curl ca-certificates
  Arch:         pacman -S --needed base-devel git curl ca-certificates
  Alpine:       apk add build-base pkgconf git curl ca-certificates rust cargo
  openSUSE:     zypper install gcc make pkg-config git curl ca-certificates
Also install a current stable Rust toolchain (cargo and rustc), for example
using the official instructions at https://rustup.rs/ if your packages are old.
Then rerun this command. The installer does not install dependencies itself.
EOF
    exit 1
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/handwave-build.XXXXXXXX")
staged=
cleanup() {
    [ -z "$staged" ] || rm -f -- "$staged"
    rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
archive=$work/source.tar.gz
printf '%s\n' 'Downloading the pinned HandWave source bundle…'
if [ "$downloader" = curl ]; then
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 20 --max-time 300 --output "$archive" "$source_url" || fail 'Source download failed; installation unchanged.'
else
    wget --https-only --timeout=30 --tries=2 -q -O "$archive" "$source_url" || fail 'Source download failed; installation unchanged.'
fi
actual=$(sha256sum "$archive")
actual=${actual%% *}
# Accept the conventional upper-case digest as well.
checksum=$(printf '%s' "$checksum" | tr 'A-F' 'a-f')
[ "$actual" = "$checksum" ] || fail 'Source SHA-256 mismatch; installation unchanged.'

# Only ordinary files/directories under one fixed root are accepted. Reject
# links, device nodes, traversal and escaped/ambiguous archive names before tar.
tar -tzf "$archive" > "$work/names" || fail 'Invalid source archive.'
awk '
  $0 !~ /^handwave-node\/?$/ && $0 !~ /^handwave-node\/[A-Za-z0-9_.@+\/=-]+$/ { exit 1 }
  $0 ~ /(^|\/)\.\.?($|\/)/ { exit 1 }
  END { if (NR == 0) exit 1 }
' "$work/names" || fail 'Unsafe source archive paths.'
tar -tvzf "$archive" > "$work/types" || fail 'Invalid source archive metadata.'
awk 'substr($0,1,1) != "-" && substr($0,1,1) != "d" { exit 1 }' "$work/types" \
    || fail 'Source archive contains links or special files.'
mkdir "$work/source"
tar -xzf "$archive" -C "$work/source" --no-same-owner --no-same-permissions || fail 'Source extraction failed.'
source_dir=$work/source/handwave-node
[ -f "$source_dir/Cargo.toml" ] && [ -f "$source_dir/Cargo.lock" ] \
    && [ -f "$source_dir/node.conf.example" ] || fail 'Source bundle is missing Cargo.toml, Cargo.lock or node.conf.example.'
printf '%s\n' 'Building locally (the first build can take several minutes)…'
(
    cd "$source_dir"
    CARGO_TARGET_DIR="$work/target" cargo build --release --locked
) || fail 'Compilation failed; existing installation and configuration are unchanged.'
built=$work/target/release/handwave-node
[ -f "$built" ] && [ -x "$built" ] || fail 'Build did not produce the handwave-node executable.'

# Compile first, then stage beside the destination for a same-filesystem rename.
mkdir -p "$prefix"
check_destination
staged=$(mktemp "$prefix/.handwave-node.XXXXXXXX")
cp "$built" "$staged"
chmod 755 "$staged"
if [ ! -e "$prefix/node.conf.example" ] && [ ! -L "$prefix/node.conf.example" ]; then
    cp "$source_dir/node.conf.example" "$prefix/node.conf.example"
fi
mv -f -- "$staged" "$prefix/handwave-node"
staged=
printf '\nInstalled: %s/handwave-node\n' "$prefix"
printf '%s\n' 'No device was connected or approved automatically.'
printf 'Next: copy "%s/node.conf.example" to "%s/node.conf" if you have no configuration,\n' "$prefix" "$prefix"
printf '%s\n' 'set your HandWave enrollment token, then run:'
printf '  "%s/handwave-node" --cwd /absolute/path/to/project\n' "$prefix"
printf '%s\n' 'Approve the new device in the authenticated HandWave workbench. Existing node.conf and state are unchanged.'
