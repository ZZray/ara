# Product integration boundary

ARA Core is shared code. AI HandWave, [Lantern](https://github.com/ZZray/Lantern) adapted from [Paseo](https://github.com/getpaseo/paseo), Lumen, and later personal AI projects are separate hosts or frontends. The named integrations are planned, not currently implemented by this new repository.

Each host keeps its own product identity, storage location, permissions, UI state, and lifecycle. It binds the same Core package through narrow native/RPC interfaces with typed content and event streams. Shared code does not imply shared database, Session, accepted Task, or cross-product authority. A frontend may display and control a Run only through the host's checked APIs.

Lantern can periodically compare a **specific** Paseo upstream commit with its previous adapted commit, much like the OMP Core sync process. Record source, changes, local differences, and real UI tests. Compatibility with OMP wire frames can reduce adaptation work, but it is not proof that Lantern already works with ARA.

Product acceptance needs a real host and UI flow: first use, tool permission, streamed progress, cancellation, failure feedback, history resume, and distinct identities. A standalone Core fixture proves only its own layer. See [verification](verification.md).
