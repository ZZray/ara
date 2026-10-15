# Linux source installer delivery, 2026-10-15

Scope: compact Rust node source distribution and the authenticated Ray install
entry. No Rust Agent runtime, cloud memory, model protocol or admission behavior
was changed. Root ARA/OMP acceptance counters are unrelated.

## Versioned artifacts

- Source SHA-256: `b906c8a6ea46ae169736d45c65b526a987a1e2d3e177de9c5cd0c50a5c126aeb`.
- Parametric installer SHA-256: `fe4b10bee6a73659f6c5db78e624aa794bec0e0b57f91619d8be13c4f694094c`.
- Bundle: 267,556 bytes, 578 explicit source files including 545 license files.
- Rust dependencies retain Cargo.lock and pi-rust revision
  `8ec462672d90e62712d96074638efef89436cebe`.
- Source is a frozen release snapshot. This later evidence record is outside it.
  Private configuration, node state, dist and target are excluded.
- Packaging owner: [package-source.py](../../scripts/package-source.py).
  Installer owner: [install-linux.sh](../../scripts/install-linux.sh).

The authenticated Ray page is `/ray/install`; the root portal and original
HandWave download page link to it. It presents:

```sh
wget -O install.sh https://118.25.103.226/ray/install.sh
chmod +x install.sh
./install.sh
```

The stable script route serves the reviewed installer with fixed source URL and
source SHA defaults. No credentials are embedded. It is one exact GET/HEAD
public path with no-store, alongside two exact versioned artifact paths.
Install-page access remains gated by the existing platform session. Source
downloads do not create or approve a device.

## Validation and preserved failures

Independent Codex PRE/POST and final-short-command review applied
`ara-git-review` to the exact named files in the two dirty working trees.
Review caught missing recursive license inclusion, directory-target false
success, dangling example links and Alpine GNU-wget assumptions. Corrections
preserve existing data; all notice references in the bundle resolve. Initial
Go page assertion failed because HTML escapes plus signs; the test now decodes
HTML entities rather than changing the correct rendered command.

- Go package tests, vet and Linux release build pass on the delivered Ray code.
  Four independent focused endpoint/page/default-command tests pass.
- 17 controlled shell tests cover failures, archive traversal/links, missing
  lockfile/dependencies, existing config/state, binary-directory/link rejection,
  reinstall and a prefix containing spaces. Fake compiler/downloader tests are
  not real compilation evidence.
- Actual packaged source built with `cargo build --release --locked` on
  WSL x86_64 GNU Linux / Rust 1.97.1; version output is
  `handwave-node 0.1.0`.
- Actual production long-command download/checksum/fresh-target compilation and
  installation passed in 99.252 seconds. Only an isolated installation prefix
  and dependency cache were supplied; no fake compiler/downloader or Hub
  connection. Binary SHA:
  `a2059b86cdaaf07f43e077638ae3ac0ba5a68957929ba2252a8885f13c57546b`,
  5,266,000 bytes. No live node.conf was generated.
- Actual production three-line command also passed a separate fresh-target
  build/install in 112.356 seconds, with the same binary SHA and version. Only
  an isolated prefix and set-eu wrapper were added to the rendered commands.
  The private `short-live-installer.json` receipt records this independent result.
- Browser checked authenticated navigation, source card, three-line rendering,
  375px no horizontal document overflow and keyboard dependencies disclosure.
  Copy-success feedback appeared; the browser tool's virtual clipboard was
  empty, so actual clipboard paste is not claimed.
- Light/dark composer backgrounds are unchanged by focus after the CSS repair.

Final Ray binary SHA:
`e98596bd87a3c4135c65c165964cc9b72d793bda38d4bade28308f833ac66cee`.
Only Ray was replaced; private configuration/authentication hashes and the five
upstream service process identities were preserved. Private receipts, logs and
screenshots: `handwave-source-install-20261015`.

## Boundaries

ARM64, musl/Alpine, older Rust versions and other distributions were not actually
compiled in this task. Scripts report prerequisites but do not install them or
elevate privileges. Installation does not start a service, connect a new device,
supply enrollment tokens, replace state or prove runtime/cloud task acceptance.
First builds can take minutes; dependency-cache timings are not speed benchmarks.
