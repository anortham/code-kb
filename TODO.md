# TODO

- [x] **GitHub Actions release binaries:** Matrix workflow in `.github/workflows/release-binaries.yml` for Linux (`x86_64-unknown-linux-gnu`), Windows (`x86_64-pc-windows-msvc`), macOS Intel (`x86_64-apple-darwin`), and macOS Apple Silicon (`aarch64-apple-darwin`), plus `.github/workflows/ci.yml`.
- [x] **MCP server instructions & tool adoption:** Tuned tool descriptions with progressive disclosure contrast keywords; verified `initialize` instructions.
- [x] **Hooks and/or skills:** Created token-dense agent skill `skills/code-kb/SKILL.md` (and `.claude-plugin/skills/code-kb/SKILL.md`).
- [x] **License files:** Added `LICENSE-MIT` and `LICENSE-APACHE` to repository root; aligned `Cargo.toml` to `MIT OR Apache-2.0`.
- [x] **Extractor distribution strategy:** Release workflow bundles matching `julie-extract` binary from `anortham/julie-extractors` into release archives.
- [ ] **Crates.io publication:**
  - Publish `code-kb-core` v0.1.0 (`cargo publish -p code-kb-core`).
  - Publish `code-kb-cli` v0.1.0 (`cargo publish -p code-kb-cli`).
