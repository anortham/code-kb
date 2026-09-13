## Review answers

1. **GitHub Actions release binaries**

   Recommendation: use four native GitHub-hosted jobs: `ubuntu-24.04` / `x86_64-unknown-linux-gnu`, `windows-2025` / `x86_64-pc-windows-msvc`, `macos-15-intel` / `x86_64-apple-darwin`, and `macos-15` / `aarch64-apple-darwin`. GitHub documents the available runner labels and architectures in its [hosted-runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). Run native CLI scan, query, edit, and stdio smoke tests in every job before creating a release.

   Each archive should contain the matching `code-kb` binary, a tested and pinned `julie-extract` binary, license notices, and a checksum. No workflow exists yet. `julie-extract` 2.42.0 is installed locally, but compatibility across these four targets is unverified.

2. **MCP adoption**

   Guidance: short `initialize` instructions are sufficient; the tools already support progressive disclosure. Keep tool descriptions concrete and compact, then dogfood them with agents before adding more guidance.

3. **Hooks or skills**

   Recommendation: add neither initially. Consider a compact usage skill only if dogfooding shows agents consistently overlook the tools. No hook has a demonstrated need.

4. **Publishing `julie-extract` with `code-kb`**

   Not ready. The CLI is blocked until the versioned `code-kb-core` 0.1.0 dependency is published, and no extractor is bundled. `cargo package -p code-kb-core --allow-dirty --no-verify` succeeds locally, which only proves the core package can be assembled.

   Release decisions still needed: choose one license policy because the workspace Cargo metadata is MIT while the plugin metadata is dual MIT/Apache, and add the corresponding license files. Then validate package contents, pinned extractor provenance, all four native binaries, and the unbenchmarked memory, latency, and token targets.

5. **Bounded memory**

   Review opportunity: the less-than-15 MB retained-memory target is unproved. `outline` can retain O(files at the requested depth), and the public `ReconcileReport` retains O(changed paths). Choose output and report-capping semantics before claiming the target; this is a design decision, not an implemented correction.

6. **Standard MCP roots**

   Review opportunity: standard MCP `roots/list` round-trips are unsupported; binding currently relies on CWD, `--root`, absolute paths, or legacy initialization hints. The MCP [client roots specification](https://modelcontextprotocol.io/specification/2024-11-05/client/roots) defines the standard exchange. Support it if hosts may launch outside the repository; otherwise document explicit `--root` setup. This is a feature recommendation, not an implemented correction.
