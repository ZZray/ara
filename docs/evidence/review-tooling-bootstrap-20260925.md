# Review tooling bootstrap, 2026-09-25

This record describes preparation, not a review or test of Rust backend code. The new repository has no `Cargo.toml` or Rust implementation at this snapshot.

| Source | Local observation | Repo use |
| --- | --- | --- |
| Existing user-level `code-reviewer` and `ocr-code-review` Skills | Read for exact-version scope, coverage, evidence, and read-only review semantics; not copied verbatim. | Adapted into `ara-git-review`. |
| [Alibaba Open Code Review](https://github.com/alibaba/open-code-review) | `ocr --version`: v1.12.5; existing local command, no credentials bundled. | Optional deterministic diff/rule selection. No automated model reviewer. |
| [Rust Clippy usage](https://doc.rust-lang.org/clippy/usage.html) | `rustup component add clippy`; `cargo clippy --version`: 0.1.97. | Future Rust backend lint on actual workspace. |
| [Cargo test](https://doc.rust-lang.org/cargo/commands/cargo-test.html) | Cargo 1.97.1 available; no Rust manifest yet. | Future actual unit/integration/doc and host tests. |
| [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) | Official 0.20.2 Windows release installed locally; archive SHA-256 `975A22143262FD27476D19EE00C7AF67978426E40E1DEE94EED6BBADE1CF87DC` matched its companion checksum; `cargo deny --version` returned 0.20.2. | Future dependency advisory/license/source check after a reviewed policy and lockfile exist. |

The online OpenAI `security-best-practices` Skill was inspected but not imported: its stated supported languages are Python, JavaScript/TypeScript, and Go, not Rust. The repository Skills were written for ARA's Rust Core and host contracts using the local review lessons and the official tools above. No external Skill text, executable, API key, or model configuration was committed. Tool presence does not prove any backend behavior; later evidence must name the exact code snapshot and commands run.

For this bootstrap diff, `ocr delegate preview --format json` selected the workflow and two Python scripts (3 code entries) and excluded 11 Markdown entries as unsupported; the Markdown/Skill content was checked through the Skill validator and manual scope review. This OCR preview is scope evidence, not an independent semantic review or backend test.
