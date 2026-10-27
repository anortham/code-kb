# Task 5: Java and C# case readiness

## Cases

- `commons-lang` is a migration task pinned to `a5d6c5ed8847b2b14dacf062b646f94d6cc61a10`. It moves null-safe compare and varargs min/max APIs into `ComparableUtils`, retains `ObjectUtils` compatibility delegates, and preserves comparisons against a base `Comparable` type. The hidden contract has five tests and the reference patch changes exactly `ObjectUtils.java` and `ComparableUtils.java`.
- `nodatime` is a feature task pinned to `649ce5a1c8ddf439eaed7b3bf9157882363cb937`. It adds calendar-neutral physical-day comparison to `LocalDate` and day-then-time comparison to `LocalDateTime`, retaining the existing calendar-sensitive operations. The hidden contract has two tests and the reference patch changes exactly `LocalDate.cs` and `LocalDateTime.cs`.
- Neither task describes a planted change as an upstream defect. Both use empty seed patches because the requested API is absent from the pinned sources.
- The source snapshots are Apache-2.0 licensed. Eligibility is 264 Java production files / 105,736 lines for Commons Lang and 181 C# files / 34,861 lines for NodaTime. The counting rules are recorded in each case; the NodaTime count excludes the tracked calculator file whose source identifies generated members.

## Readiness results

- Commons Lang baseline public regressions passed: `ComparableUtilsTest` and `ObjectUtilsTest`, 130 tests. The seeded hidden check failed with the expected missing-symbol compiler diagnostic. The reference passed the same 130 public tests and all five hidden contract tests.
- NodaTime baseline public regressions passed: `LocalDateTest` and `LocalDateTimeTest`, 248 tests on `net10.0`. The seeded hidden check failed because `CompareToIgnoringCalendar` is absent. The reference passed the same 248 public tests and both hidden contract tests.
- Final retained logs are under `/home/murphy/.cache/code-kb-task5-20261027/logs`: Commons baseline `commons-baseline-v2-public-test`, seed failure `commons-seed-v3-hidden-test`, reference public `commons-reference-public-test`, and reference hidden `commons-reference-hidden-v2-test`; NodaTime baseline `nodatime-baseline-v5-public-test`, seed failure `nodatime-seed-hidden-test`, reference public `nodatime-reference-public-test`, and reference hidden `nodatime-reference-hidden-test`. Each has separate `stdout.log` and `stderr.log`.
- The baseline and reference-public project snapshots were verified without the injected hidden test. The seed and reference-hidden snapshots contain the test at its protected destination. The successful outputs include test-runner discovery and test names/counts, not only exit status.
- Both reference patches pass `git apply --check` on their clean pinned source trees and modify the two declared production files. `check_edit` against base `393ba33990fc12505f6fa7108d11a157d740c725` reported no changed symbol shape in the evaluation repository at 2026-10-27 14:54:42 UTC.

## Tool and dependency bindings

- Java runs use Temurin JDK `21.0.13+9` (official archive SHA-256 `d7f5d5cf865a2902b70ff549b97586a7d3f234e016f041ab447ea740c7d66d1c`) and Apache Maven `3.10.0` (archive SHA-512 `908b1501bfb420bf7c8affb855534a9c407fd6099367bfb9f2f2dcb8e9799102bffb84518cde74c679bd76870247c6528683abdd620581bffa90f95d92d175aa`). Both tools are mounted from `/tmp/code-kb-task5-20261027/tools`.
- The first Commons readiness attempt with installed OpenJDK 25 failed because the project targets Java 8. The retained final checks use the pinned Temurin 21 toolchain; the original failure logs remain in the same log root. Maven dependency preparation and the additional Surefire JUnit Platform provider fetch passed in the reviewed network-enabled preparation sandbox. The canonical Maven cache is `/tmp/code-kb-task5-20261027/dependencies/commons-m2`.
- NodaTime uses installed .NET SDK `10.0.113` from `/usr/lib64/dotnet`, matching its `global.json` policy (`10.0.101`, latest-minor roll-forward) and selected `net10.0` test target. NuGet restore passed in preparation, and the final runtime restore passed from the canonical `/tmp/code-kb-task5-20261027/dependencies/nodatime-nuget` cache with network disabled.
- NodaTime's sandboxed build needed the command-line property `-p:PublicSign=true`: private-key strong-name signing failed in the sandbox's OpenSSL environment, while disabling signing broke the test project's friend-assembly identity. Public signing preserves the public-key identity used by the tests. The test run therefore validates compilation and behavior but does not validate a private-key signature artifact.
- Commons' hidden-only command uses `-Drat.skip=true` because the harness-injected test intentionally has no upstream license header; the hidden compile/behavior contract still runs, and both public commands retain the normal RAT check.
- Exact final host input paths, tool/dependency tree digests, source revisions, version logs, and preparation log paths are in ignored `target/edit-evaluation/task5/runtime-bindings.json`. Sandbox run logs remain outside all mounts.

## Isolation and remaining limits

- No upstream setup, build, or test command ran on the host. Preparation and readiness commands ran through the reviewed sandbox. No global tool or configuration was installed or changed.
- Each case protects canonical tests/configuration individually, reserves the hidden destination, keeps build outputs out of templates, and declares the case's tool/dependency mount names and generated roots. Commons has 392 protected paths; NodaTime has 298.
- The only unverified property is private-key signing for NodaTime; its feature and selected public regressions are covered by the public-sign sandbox run. The source snapshots and case/test contract should still receive the lead's final integration review before any study dispatch.

## Repository state

- Worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`; branch `fix/cross-language-review`; HEAD `7d7073a8b73a91fb98bf98d002d2247c5fc815b0` at final verification.
- The tree is dirty with the two case directories and sibling-owned `scripts/edit_evaluation.py` / `scripts/test_edit_evaluation.py` untracked. No files were staged or committed. The two pinned upstream source snapshots remain clean.
