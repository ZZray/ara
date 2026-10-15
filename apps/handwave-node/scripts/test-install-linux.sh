#!/bin/sh
# Controlled fixtures exercise installation boundaries, not a real Rust build.
set -eu
installer=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/install-linux.sh
test_root=$(mktemp -d "${TMPDIR:-/tmp}/handwave-installer-test.XXXXXXXX")
trap 'rm -rf -- "$test_root"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$test_root/bin" "$test_root/source/handwave-node" "$test_root/install"
printf '[package]\nname="fixture"\n' > "$test_root/source/handwave-node/Cargo.toml"
printf '# locked fixture\n' > "$test_root/source/handwave-node/Cargo.lock"
printf 'hub=https://example.invalid/\ntoken=YOUR_TOKEN\n' > "$test_root/source/handwave-node/node.conf.example"
tar -czf "$test_root/source.tar.gz" -C "$test_root/source" handwave-node
digest=$(sha256sum "$test_root/source.tar.gz"); digest=${digest%% *}
cat > "$test_root/bin/curl" <<'EOF'
#!/bin/sh
[ "${DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
while [ "$#" -gt 0 ]; do
    if [ "$1" = --output ]; then cp "$FIXTURE_ARCHIVE" "$2"; exit 0; fi
    shift
done
exit 1
EOF
cat > "$test_root/bin/cargo" <<'EOF'
#!/bin/sh
[ "$*" = 'build --release --locked' ] || exit 12
[ "${BUILD_FAIL:-0}" = 0 ] || exit 13
mkdir -p "$CARGO_TARGET_DIR/release"
printf '#!/bin/sh\necho fixture-built\n' > "$CARGO_TARGET_DIR/release/handwave-node"
chmod 755 "$CARGO_TARGET_DIR/release/handwave-node"
EOF
cat > "$test_root/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo "${FIXTURE_OS:-Linux}" ;; -m) echo "${FIXTURE_ARCH:-x86_64}" ;; *) exit 1 ;; esac
EOF
for command in cc pkg-config git; do printf '#!/bin/sh\nexit 0\n' > "$test_root/bin/$command"; done
chmod 755 "$test_root/bin/"*
PATH=$test_root/bin:$PATH
FIXTURE_ARCHIVE=$test_root/source.tar.gz
export PATH FIXTURE_ARCHIVE
passed=0
run_install() { sh "$installer" --source-url https://example.invalid/source.tar.gz --sha256 "$digest" --prefix "$test_root/install" "$@" > "$test_root/output" 2>&1; }
pass() { passed=$((passed + 1)); printf 'PASS %s\n' "$1"; }
expect_failure() { if run_install "$@"; then cat "$test_root/output"; echo 'Expected failure' >&2; exit 1; fi; }
check_preserved() {
    [ "$(cat "$test_root/install/handwave-node")" = old-binary ]
    [ "$(cat "$test_root/install/node.conf")" = private-config ]
    [ "$(cat "$test_root/install/state/identity")" = existing-identity ]
}
printf 'old-binary\n' > "$test_root/install/handwave-node"
printf 'private-config\n' > "$test_root/install/node.conf"
mkdir "$test_root/install/state"
printf 'existing-identity\n' > "$test_root/install/state/identity"
sh "$installer" --help > "$test_root/help"
pass help
expect_failure --source-url http://example.invalid/source.tar.gz
check_preserved; pass reject-http
expect_failure --sha256 wrong
check_preserved; pass reject-invalid-digest
expect_failure --sha256 0000000000000000000000000000000000000000000000000000000000000000
check_preserved; pass checksum-failure-preserves-installation
DOWNLOAD_FAIL=1; export DOWNLOAD_FAIL
expect_failure
unset DOWNLOAD_FAIL
check_preserved; pass download-failure-preserves-installation
BUILD_FAIL=1; export BUILD_FAIL
expect_failure
unset BUILD_FAIL
check_preserved; pass compile-failure-preserves-installation
FIXTURE_ARCH=riscv64; export FIXTURE_ARCH
expect_failure
unset FIXTURE_ARCH
check_preserved; pass reject-unsupported-architecture
mkdir "$test_root/minimal-bin"
for command in uname tar sha256sum cc cargo git awk tr mktemp cp chmod mv mkdir rm curl cat; do
    ln -s "$(command -v "$command")" "$test_root/minimal-bin/$command"
done
if PATH="$test_root/minimal-bin" /bin/sh "$installer" \
    --source-url https://example.invalid/source.tar.gz --sha256 "$digest" \
    --prefix "$test_root/install" > "$test_root/output" 2>&1; then
    echo 'Expected missing dependency failure' >&2; exit 1
fi
grep -q 'Missing build tools: pkg-config' "$test_root/output"
grep -q 'Debian/Ubuntu:' "$test_root/output"
check_preserved; pass missing-dependency-includes-install-guidance
ln -s /etc/passwd "$test_root/source/handwave-node/unsafe-link"
tar -czf "$test_root/unsafe.tar.gz" -C "$test_root/source" handwave-node
FIXTURE_ARCHIVE=$test_root/unsafe.tar.gz
unsafe_digest=$(sha256sum "$FIXTURE_ARCHIVE"); unsafe_digest=${unsafe_digest%% *}
expect_failure --sha256 "$unsafe_digest"
check_preserved; pass reject-archive-symlink
rm "$test_root/source/handwave-node/unsafe-link"
tar -czf "$test_root/traversal.tar.gz" --transform='s#^handwave-node#handwave-node/../escape#' -C "$test_root/source" handwave-node
FIXTURE_ARCHIVE=$test_root/traversal.tar.gz
unsafe_digest=$(sha256sum "$FIXTURE_ARCHIVE"); unsafe_digest=${unsafe_digest%% *}
expect_failure --sha256 "$unsafe_digest"
check_preserved; pass reject-archive-traversal
mv "$test_root/source/handwave-node/Cargo.lock" "$test_root/held-lock"
tar -czf "$test_root/incomplete.tar.gz" -C "$test_root/source" handwave-node
FIXTURE_ARCHIVE=$test_root/incomplete.tar.gz
unsafe_digest=$(sha256sum "$FIXTURE_ARCHIVE"); unsafe_digest=${unsafe_digest%% *}
expect_failure --sha256 "$unsafe_digest"
check_preserved; pass reject-missing-lockfile
mv "$test_root/held-lock" "$test_root/source/handwave-node/Cargo.lock"
FIXTURE_ARCHIVE=$test_root/source.tar.gz
run_install
[ "$("$test_root/install/handwave-node")" = fixture-built ]
[ "$(cat "$test_root/install/node.conf")" = private-config ]
[ "$(cat "$test_root/install/state/identity")" = existing-identity ]
[ -f "$test_root/install/node.conf.example" ]
pass successful-atomic-install-preserves-configuration-and-state
printf 'custom-example\n' > "$test_root/install/node.conf.example"
run_install
[ "$(cat "$test_root/install/node.conf.example")" = custom-example ]
pass reinstall-preserves-existing-example
mv "$test_root/install/handwave-node" "$test_root/held-binary"
mkdir "$test_root/install/handwave-node"
expect_failure
[ -z "$(ls -A "$test_root/install/handwave-node")" ]
[ -z "$(find "$test_root/install" -name '.handwave-node.*' -print)" ]
rmdir "$test_root/install/handwave-node"
pass reject-directory-target-without-writing-into-it
mkdir "$test_root/linked-target"
ln -s "$test_root/linked-target" "$test_root/install/handwave-node"
expect_failure
[ -z "$(ls -A "$test_root/linked-target")" ]
[ -L "$test_root/install/handwave-node" ]
rm "$test_root/install/handwave-node"
pass reject-symlink-directory-target-without-writing-into-it
ln -s "$test_root/held-binary" "$test_root/install/handwave-node"
expect_failure
[ "$("$test_root/held-binary")" = fixture-built ]
rm "$test_root/install/handwave-node"
pass reject-symlink-file-target
run_install --prefix "$test_root/install with spaces"
[ "$("$test_root/install with spaces/handwave-node")" = fixture-built ]
[ -f "$test_root/install with spaces/node.conf.example" ]
pass installation-prefix-with-spaces
printf '%s controlled installer checks passed. No real model or Hub connection was used.\n' "$passed"
