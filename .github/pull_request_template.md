## What and why

<!-- What does this PR change, and why? Link the issue: "Closes #123". -->

## Commands / spec

<!-- CommandIds added or changed, and the docs/spec/features.md rows updated. Write "none" if this does not apply. -->

## How it was tested

<!-- Tests added, commands run, databases tried. Attach a screenshot for UI changes. -->

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace --exclude thaan-app` passes
- [ ] Logic is in `thaan-core` or a driver crate, not in `thaan-app`
- [ ] `docs/spec/features.md` is updated (behaviour and status)
- [ ] No secrets, credentials or private data in code, fixtures, logs or screenshots
- [ ] Commits are signed off (`git commit -s`)
- [ ] Breaking change? If yes, the title has `!` and the change is explained above

## Clean-room

- [ ] Clean-room: written from docs/spec and public docs only.
