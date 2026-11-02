# Third-party software

RY SSH Tunnel is maintained by Ray Zhang. Original copyright notices remain in the accompanying license files.

| Component | Version | Source | License copy |
|---|---|---|---|
| Go toolchain and standard library | 1.26.5 | https://go.dev/ | `licenses/Go.txt` |
| coder/websocket | 1.8.14 | https://github.com/coder/websocket | `licenses/coder-websocket.txt` |
| golang.org/x/crypto | 0.57.0 | https://go.googlesource.com/crypto | `licenses/x-crypto.txt` |
| golang.org/x/sys | 0.48.0 | https://go.googlesource.com/sys | `licenses/x-sys.txt` |
| github.com/google/uuid | v1.6.0 | https://github.com/google/uuid | `licenses/github.com-google-uuid.txt` |
| github.com/pion/datachannel | v1.6.3 | https://github.com/pion/datachannel | `licenses/github.com-pion-datachannel.txt` |
| github.com/pion/dtls/v3 | v3.1.15 | https://github.com/pion/dtls | `licenses/github.com-pion-dtls-v3.txt` |
| github.com/pion/ice/v4 | v4.4.8 | https://github.com/pion/ice | `licenses/github.com-pion-ice-v4.txt` |
| github.com/pion/interceptor | v0.1.51 | https://github.com/pion/interceptor | `licenses/github.com-pion-interceptor.txt` |
| github.com/pion/logging | v0.2.4 | https://github.com/pion/logging | `licenses/github.com-pion-logging.txt` |
| github.com/pion/mdns/v2 | v2.2.2 | https://github.com/pion/mdns | `licenses/github.com-pion-mdns-v2.txt` |
| github.com/pion/randutil | v0.1.0 | https://github.com/pion/randutil | `licenses/github.com-pion-randutil.txt` |
| github.com/pion/rtcp | v1.2.20 | https://github.com/pion/rtcp | `licenses/github.com-pion-rtcp.txt` |
| github.com/pion/rtp | v1.10.6 | https://github.com/pion/rtp | `licenses/github.com-pion-rtp.txt` |
| github.com/pion/sctp | v1.12.0 | https://github.com/pion/sctp | `licenses/github.com-pion-sctp.txt` |
| github.com/pion/sdp/v3 | v3.0.20 | https://github.com/pion/sdp | `licenses/github.com-pion-sdp-v3.txt` |
| github.com/pion/srtp/v3 | v3.1.3 | https://github.com/pion/srtp | `licenses/github.com-pion-srtp-v3.txt` |
| github.com/pion/stun/v4 | v4.0.1 | https://github.com/pion/stun | `licenses/github.com-pion-stun-v4.txt` |
| github.com/pion/transport/v5 | v5.1.4 | https://github.com/pion/transport | `licenses/github.com-pion-transport-v5.txt` |
| github.com/pion/turn/v5 | v5.1.3 | https://github.com/pion/turn | `licenses/github.com-pion-turn-v5.txt` |
| github.com/pion/webrtc/v4 | v4.2.26 | https://github.com/pion/webrtc | `licenses/github.com-pion-webrtc-v4.txt` |
| github.com/wlynxg/anet | v0.0.5 | https://github.com/wlynxg/anet | `licenses/github.com-wlynxg-anet.txt` |
| golang.org/x/net | v0.58.0 | https://golang.org/x/net | `licenses/golang.org-x-net.txt` |
| golang.org/x/time | v0.14.0 | https://golang.org/x/time | `licenses/golang.org-x-time.txt` |
| Win32-OpenSSH | 10.0.0.0p2-Preview | https://github.com/PowerShell/Win32-OpenSSH/releases | `payload/OpenSSH-Win64/LICENSE.txt` and `NOTICE.txt` in distribution packages |

The Windows binary payload comes from the user-provided offline archive. Its SHA-256 is pinned by the packaging script. The original archive's `authorized_keys`, installation wrapper and nested ZIP are excluded. Review the upstream release and payload before changing the pinned hash. OpenSSH binaries are distributed in the ZIP packages and are not checked into this repository.
