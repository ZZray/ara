# portable-pty 0.9.0 in ARA

Source: exact crates.io portable-pty 0.9.0 archive.
Archive SHA256: b4a596a2b3d2752d94f51fac2d4a96737b8705dddd311a32b9af47211f08671e.
All 17 official archive files and their upstream license/metadata are retained.

The only upstream source change is `src/win/psuedocon.rs::load_conpty`:
use the existing system kernel32 ConPTY exports, without automatic bare-name
`conpty.dll` sideload. The upstream bare LoadLibraryW search may load a DLL from
the current project or PATH, executing its entry point before export validation.
ARA does not ship a paired sideloaded conpty.dll/openconsole.exe distribution.
Support for that optional console distribution is not claimed.

The public ABI, flags, create/resize/close, pipes, spawning, I/O and Unix source
are unchanged. This is a documented Windows Host dependency adaptation to the
fixed OMP b91db66e source, which also uses portable-pty. It is not a claim that
the upstream loader was different or that complete OMP/WRITE has been accepted.

The workspace patch pins this exact local source. It is excluded as a workspace
member; its examples/dev dependencies are not module verification targets.
Windows/Linux actual-process module banks and independent source/graph reviews
are required before accepting HOST-SERVICE-CONTROL. No MSRV 1.88 build is implied.
