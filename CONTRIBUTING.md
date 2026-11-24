# Contributing to ThaanDB

Thanks for helping. This file covers the rules for issues, code and pull requests.
For the project layout, principles and commands, read [`AGENTS.md`](AGENTS.md) first.

## Licence and sign-off

- ThaanDB is licensed under the [Apache License 2.0](LICENSE). By opening a pull request
  you agree that your contribution is released under the same licence (Apache-2.0,
  section 5). There is no separate CLA.
- Every commit must carry a `Signed-off-by:` line. Add it with `git commit -s`. The
  sign-off certifies the [Developer Certificate of Origin](https://developercertificate.org/):
  you wrote the change, or you have the right to submit it under this licence.
- New dependencies must use a permissive licence (MIT, Apache-2.0, BSD, Zlib, ISC, or
  MPL-2.0 as a dependency). GPL / AGPL / LGPL dependencies are not accepted.

## Clean-room rule (required)

ThaanDB is written clean-room. Before contributing, read
[`docs/clean-room.md`](docs/clean-room.md). In short:

- Never read, unpack, decompile or reverse-engineer another database client's code or
  binaries for this project, whether it is proprietary or open source. This includes
  Electron `app.asar` bundles and source maps.
- Never copy code, SQL templates, CSS, icons, strings or trademarks from another product.
- You may describe another client's *behaviour* (what you see as a user) in
  `docs/spec/`, in your own words.
- If you have read another client's source for an area, do not implement that area here,
  and say so in the PR.

A pull request that breaks this rule is closed, however useful it is.

## Before you start

- **Bugs:** open an issue with the version or commit, OS, database and version, steps to
  reproduce, and what you expected. Do not include passwords, connection strings with
  credentials, or private data.
- **Features:** open an issue or a draft PR that adds or updates the row in
  [`docs/spec/features.md`](docs/spec/features.md) first, so the behaviour and the
  `CommandId` can be agreed before the code is written.
- **Security issues:** do not open a public issue. Contact the maintainers privately.

## Making a change

Follow the steps in `AGENTS.md`. The main points are:

1. Put logic in the engine (`thaan-core` or a driver crate), not in `thaan-app`.
2. Each user-visible action is a command with a stable `CommandId` and typed parameters,
   registered in `builtin_commands()`. Never rename a `CommandId`; deprecate it instead.
3. SQL text comes from the `Dialect`. Identifiers are quoted by the dialect, and values are
   always bound parameters.
4. Add tests. Engine behaviour is tested against SQLite in-memory databases. Tests that
   need a live server are `#[ignore]` and read their URL from `THAAN_TEST_<NAME>_URL`.
5. Do not use `unwrap()` / `expect()` outside tests, except on an invariant with a comment.
6. Update the status column in `docs/spec/features.md` when a feature lands.

## Checks

A pull request must pass all of these locally:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude thaan-app
```

If you touched `thaan-app`, also run `cargo build -p thaan-app` and check the change in
the app. For UI changes, attach a screenshot.

## Pull request rules

- **One topic per PR.** Keep it small and focused. Split refactors from behaviour changes.
- **Branch** from `main` and name it `<type>/<short-topic>`, for example
  `feat/postgres-driver` or `fix/csv-export-quotes`.
- **Commit messages** follow [Conventional Commits](https://www.conventionalcommits.org/):
  `feat(core): add table.truncate command`, `fix(cli): ...`, `docs: ...`. Types are
  `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Scope is the
  crate without the `thaan-` prefix (`core`, `cli`, `app`, `driver-sqlite`, ...).
- **Title** uses the same format as the commit message.
- **Description** fills in the PR template: what and why, the linked issue, how it was
  tested, and the clean-room statement:
  *"Clean-room: written from docs/spec and public docs only."*
- **No secrets** in code, tests, fixtures, logs or screenshots.
- **Review:** at least one maintainer approval is needed. Reply to every review comment
  or resolve it. Squash-merge is the default, so the PR title becomes the commit on `main`.
- **Breaking changes** (a CLI flag, a stored config format, a command's parameters) must be
  marked with `!` in the title (`feat(core)!: ...`) and explained in the description.

## Code of conduct

Be respectful and constructive. Criticise code, not people. Maintainers may remove
comments or contributors that harass others.
