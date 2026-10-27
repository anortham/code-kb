# Pinned source and readiness evidence

- Repository: https://github.com/mitsuhiko/minijinja.git
- Pin: `81787c4eab656f3a4f4e92dea53f303a36f1685c`
- Full-history clone: `target/edit-evaluation/sources/minijinja`; clean at the pin. Root and crate licenses are retained. Clone history is not included in trial exports.
- Production eligibility: 80 tracked Rust files and 38,491 physical lines under the `src/` trees of `minijinja`, `minijinja-cabi`, `minijinja-cli`, `minijinja-contrib`, `minijinja-embed`, `minijinja-js`, and `minijinja-py`; tests, examples, benchmarks, fuzz targets, Go bindings, dependencies, and generated output are excluded.
- Production paths: `minijinja-cli/src/config.rs` (pinned autoescape callback at lines 238–249) and `minijinja-cli/src/command.rs` (pinned `--autoescape` long help at lines 239–250).
- Upstream guidance: `CONTRIBUTING.md` documents Cargo tests. `minijinja-cli/Cargo.toml` requires Rust 1.88; the locked dependency set is from `Cargo.lock`.
- Exposure screen: corpus/review files named by `corpus.json` contain no minijinja, MiniJinja, mitsuhiko, or pinned repository matches.

## Prepared inputs

- Preparation ran `cargo fetch --locked` in sandbox `prepare` mode. The Cargo cache was copied to `/tmp/code-kb-edit-python-rust-20261027/inputs/cargo-cache`, mounted as `/deps/cargo-cache`; it contains 17,514 files / 325,094,688 bytes and has tree SHA-256 `eac88f77d5490563112b892dcb38a804d9fdb32e52de2624bd5c0738faef538a`.
- `dependency_output_paths` maps `cargo-cache` to `cargo` for preparation output. Runtime setup copies the canonical cache into disposable `/state/cargo`; Cargo target output is under `/state/target`.
- Read-only tool input `rust` maps `/home/murphy/.rustup/toolchains/stable-x86_64-unknown-linux-gnu` to `/tools/rust`. Sandbox invocations report Cargo 1.97.1 and rustc 1.97.1, above the declared 1.88 minimum.
- Grade commands use `--locked --offline` and sandbox network mode is disabled. No upstream setup, build, or test command ran on the host.
- Four protected paths in the upstream tree are symlinks: `minijinja-contrib/LICENSE`, `minijinja-embed/LICENSE`, `minijinja/LICENSE`, and `minijinja/README.md`. Per the reviewed sandbox handoff, symlink binds are omitted while their paths remain in snapshot protection; the regular root `LICENSE` and `README.md` targets are protected.

## Selected checks and results

- Public command: `/usr/bin/env CARGO_HOME=/state/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/state/target CARGO_INCREMENTAL=0 RUSTC=/tools/rust/bin/rustc /tools/rust/bin/cargo test --locked --offline -p minijinja-cli --bin minijinja-cli --test test_basic -- --skip test_help`.
- The selected public targets passed on baseline and reference: 4 binary unit tests and 30 CLI tests passed; one `test_help` case was filtered. The targeted selection avoids Cargo discovering the separate hidden integration-test target.
- A full baseline `cargo test --locked --offline -p minijinja-cli` passed 4 unit and 31 CLI tests. The same full command on the reference failed only `test_help`: the protected `minijinja-cli/tests/snapshots/test_basic__long_help.snap` at `minijinja-cli/tests/test_basic.rs:895–898` records the old help sentence. The requested help change is asserted by the hidden test instead; the snapshot and test remain unchanged. The full-command logs are retained under `target/edit-evaluation/task3_python_rust/logs/minijinja-v2-*`.
- Hidden command: `/usr/bin/env CARGO_HOME=/state/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/state/target CARGO_INCREMENTAL=0 RUSTC=/tools/rust/bin/rustc /tools/rust/bin/cargo test --locked --offline -p minijinja-cli --test edit_eval_autoescape`.
- Seeded hidden check exited 101 with `legacy .j2 autoescape should remain enabled`. The repaired hidden check passed all 3 tests: automatic wrapper/output suffix detection (including uppercase `.HTML.J2` and a single-wrapper case), explicit-mode overrides, and a semantic `--long-help` assertion for all three suffixes plus case-insensitive wording in the autoescape section. The help assertion accepts equivalent phrasing.
- Final direct sandbox logs: `target/edit-evaluation/task3_python_rust/logs/minijinja-v3-seed-hidden-autoescape_suffix_acceptance/`, `target/edit-evaluation/task3_python_rust/logs/minijinja-v3-reference-hidden-autoescape_suffix_acceptance/`, `target/edit-evaluation/task3_python_rust/logs/minijinja-v4-baseline-public-minijinja_cli_regressions/`, and `target/edit-evaluation/task3_python_rust/logs/minijinja-v4-reference-public-minijinja_cli_regressions/`.
- An earlier integrated record at `target/edit-evaluation/task3_python_rust/official-readiness/minijinja/readiness.json` is retained as a failed run from the old runner revision: it ran the newly installed hidden integration test during an extra `seed_public` gate. The lead removed that incorrect gate; the final direct matrix above covers the required checks.
