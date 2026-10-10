# wrtforge: working rules

One static Rust binary that forges a clean, official OpenWrt (+ Passwall2, recovery slot) onto a router over SSH.

## Workflow (strict)

We follow gitflow, trimmed down: **`main` is the only long-lived branch. There is no `develop`.**

1. **Every task starts with a GitHub issue.** No issue, no work. Describe why, scope and "done when".
2. **Every task gets its own branch off `main`**, named `<type>/<issue#>-<short-slug>`, e.g. `feat/12-sd-card-backup`, `fix/15-ssh-timeout`, `docs/20-readme`.
3. **Every task ends in a pull request into `main`** that links the issue (`Closes #12`). Never push straight to `main`. Keep PRs small and single-purpose.
4. CI must be green before merge. Merge with **squash**, and make the squash title a Conventional Commit (it feeds the changelog and the version).
5. Merging to `main` **auto-bumps the version and publishes a release**. Do not edit versions or tags by hand.

## Commits and versions

Conventional Commits drive everything: `feat:` → minor, `fix:` / `perf:` / anything else → patch, `type!:` or a `BREAKING CHANGE:` footer → major.
Allowed types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`.

`Cargo.toml` keeps the placeholder version `0.0.0`. CI stamps the real version at build time from the git tags. Do not bump it in a PR.

## Releases and changelog

Every push to `main` that passes CI creates a git tag `vX.Y.Z` and a GitHub release with:
- a **changelog** generated from the commits since the previous tag, grouped by type and linking the PRs,
- binaries for linux (x86_64, aarch64, musl static) and macOS (x86_64, aarch64),
- `.sha256` checksums.

The changelog is only as good as the commit titles, so write them for a reader of release notes.

## Code and checks

Before opening a PR, run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- Prefer the standard library and a few small, well-known crates; do not add a dependency for something a few lines can do.
- User-visible behaviour changes update `README.md` in the same PR.
- Remote shell scripts live in `scripts/` and are embedded with `include_str!`; keep them POSIX `sh` (OpenWrt has busybox ash).
- Secrets never go in the repo: no node links, keys, passwords or router backups.

## Safety rules for the tool itself

- Back up and build a recovery path **before** flashing. Never flash without it.
- Never store or log credentials; SSH asks for the password itself.
- Anything that writes to the router stays behind an explicit confirmation, and `--dry-run` must change nothing.
