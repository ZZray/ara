# Third-party software

RY SSH Tunnel is maintained by Ray Zhang. Original copyright notices remain in the accompanying license files.

| Component | Version | Source | License copy |
|---|---|---|---|
| Go toolchain and standard library | 1.26.5 | https://go.dev/ | `licenses/Go.txt` |
| coder/websocket | 1.8.14 | https://github.com/coder/websocket | `licenses/coder-websocket.txt` |
| golang.org/x/crypto | 0.57.0 | https://go.googlesource.com/crypto | `licenses/x-crypto.txt` |
| golang.org/x/sys | 0.48.0 | https://go.googlesource.com/sys | `licenses/x-sys.txt` |
| Win32-OpenSSH | 10.0.0.0p2-Preview | https://github.com/PowerShell/Win32-OpenSSH/releases | `payload/OpenSSH-Win64/LICENSE.txt` and `NOTICE.txt` in distribution packages |

The Windows binary payload comes from the user-provided offline archive. Its SHA-256 is pinned by the packaging script. The original archive's `authorized_keys`, installation wrapper and nested ZIP are excluded. Review the upstream release and payload before changing the pinned hash. OpenSSH binaries are distributed in the ZIP packages and are not checked into this repository.
