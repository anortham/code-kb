# Claude branch review

Claude returned one medium-severity watcher finding. Lead verification accepts
a narrower queued-error case and preserves the existing bounded retry policy.
The patch is undergoing final platform verification.

## Scope and method

The requested `razorback:claude-cli` pass reviewed branch `aeebe72..08ada49`:
39 commits, 740 changed files. The complete 7,131,667-byte redacted bundle was
supplied through the skill's file transport. A fresh Claude Code 2.1.311 session
used inherited model settings, read-only Read/Grep/Glob tools, strict MCP
configuration and no session persistence. The output passed the skill's
completion-schema validator. Claude ran no tests or shell commands.

Coverage was selective: watcher registration, the debouncer backport, MCP
pagination and the evaluation sandbox. Most semantic rules and evaluation
results were not reviewed line by line. This is recorded in the
[original reviewer output](2026-10-27-claude-branch-review/reviewer.json).
No external-model policy is declared; the requested diff was sent to Anthropic.

## Finding and disposition

The accepted medium finding is a queued directory-open error. The pinned
iterator can queue a path-bearing `NotFound` before yielding a directory. If
that directory was already registered, synchronization updates its generation,
then previously propagated the queued error without using the race retry.

The fix in [watcher.rs](../../crates/code-kb-core/src/watcher.rs) rewalks only
when the error is exclusively an I/O `NotFound`, carries a registered non-root
directory path, and an independent metadata check confirms the path is gone.
It shares the existing single retry with watch-registration errors. Partial,
permission, pathless, foreign, still-present and root errors stay errors.

Two parts of Claude's explanation were not accepted:

- Deleting immediately after successful watch/yield does not cause the initial
  directory open to fail. The iterator opens it before yielding. Real native
  watcher probes on Linux and Windows both report zero walker errors and visit
  the stable sibling.
- Increasing the retry count would change the deliberate bounded-churn policy.
  The fix instead extends the existing retry to the proven missing branch.

The [pinned iterator source](https://docs.rs/walkdir/2.5.0/src/walkdir/lib.rs.html)
and [pinned error API](https://docs.rs/ignore/0.4.33/ignore/enum.Error.html)
explain the queued-error ordering and wrappers. The regression injects that
external error shape into the real registration handler deterministically;
it does not claim to reproduce the rare native syscall timing.

## Verification

The new regression first failed because the old handler propagated
`WithDepth(WithPath(Io(NotFound)))`. It covers the already-registered directory
and confirms stale registration removal. Guard tests preserve real errors and
the shared retry ceiling. A single-element partial I/O error has separate
coverage because `ignore::Error::io_error()` can expose its inner I/O error.

Linux passes all 1,910 Rust tests, Clippy with warnings denied, formatting and
the locked release build. Windows verification of the fix is pending. The
[evidence identity](2026-10-27-claude-branch-review/identity.json) binds source,
reviewer output, probes and test/build logs. The completed-edit study's
frozen binaries and results are unchanged.
