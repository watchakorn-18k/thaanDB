# Releasing

`.github/workflows/release.yml` builds ThaanDB for Windows and macOS and publishes a
GitHub release when a tag starting with `v` is pushed.

```bash
# 1. Set the version in the workspace Cargo.toml ([workspace.package] version), then
#    regenerate the Windows version info, since it is baked into app.res:
scripts/make-windows-icon.sh
# 2. Commit, tag and push the tag:
git tag v0.1.0
git push origin v0.1.0
```

What runs:

1. **Engine tests** (`cargo test --workspace --exclude thaan-app`, Ubuntu). A failure
   stops the release.
2. **Windows x64** (`windows-latest`): release build of `thaan-app` and `thaan`, packed as
   `ThaanDB-<tag>-windows-x64.zip` with `ThaanDB.exe`, `thaan.exe` and a README. The
   Windows SDK on the runner has `fxc.exe`, so GPUI compiles its shaders at build time
   (the cross-build patch in CLAUDE.md is not needed here).
3. **macOS universal** (`macos-latest`): `scripts/bundle-macos.sh --universal` builds for
   Apple Silicon and Intel and joins them with `lipo`; packed with `ditto` as
   `ThaanDB-<tag>-macos-universal.zip`.
4. **Release**: both zips and `SHA256SUMS.txt` go on the GitHub release for the tag. The
   description lists every commit since the previous tag (all commits for the first
   tag), grouped by Conventional Commit type (Features, Fixes, Performance, Other), with
   a full-changelog link: `scripts/release-notes.sh <tag>` prints the same notes
   locally. A tag with a dash (`v0.2.0-beta.1`) is marked as a pre-release.

Only a pushed `v*` tag starts it. *Run workflow* on the Actions tab (manual) runs steps
1-3 on any branch and keeps the zips as workflow artifacts, without a release.

**Cache.** Each job restores its Cargo cache (`Swatinem/rust-cache`: registry, git deps
and compiled dependencies, GPUI being most of the time) under one key per job: `tests`,
`windows-release`, `macos-universal`. GitHub scopes caches to a ref: a tag run can
restore `main`'s cache but not an earlier tag's, so tag runs only restore and the cache
is saved by runs on `main`. With tag-only triggers, that means a manual *Run workflow*
on `main`: do one now and then, mainly after `Cargo.lock` changes. When the lockfile has
changed, a tag run still restores the closest older cache and rebuilds only what differs.

Not done yet:

- **Signing.** Neither build is code-signed. Windows SmartScreen asks first (More info,
  then Run anyway). macOS says the app "cannot be checked for malicious software": right
  click the app, then Open, or run `xattr -dr com.apple.quarantine ThaanDB.app`. Run
  the `xattr` form when using the bundled MCP server (`Contents/MacOS/thaan mcp`):
  Open clears the app, but an agent starting `thaan` directly is still checked. Signing
  and notarizing need an Apple Developer ID and a Windows code-signing certificate stored
  as repository secrets.
- **Linux** builds.
- The version shown in the app comes from `Cargo.toml`, not from the tag: keep them the same.
