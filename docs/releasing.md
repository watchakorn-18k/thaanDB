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
4. **Release**: both zips and `SHA256SUMS.txt` go on the GitHub release for the tag, with
   notes generated from the commits. A tag with a dash (`v0.2.0-beta.1`) is marked as a
   pre-release.

Pushes to `main` that change `crates/`, `Cargo.toml`, `Cargo.lock`, the bundle script or
the workflow run steps 1-3 too (zips kept as workflow artifacts, no release); *Run
workflow* on the Actions tab does the same on demand.

**Cache.** Each job restores its Cargo cache (`Swatinem/rust-cache`: registry, git deps
and compiled dependencies, GPUI being most of the time) under one key per job:
`tests`, `windows-release`, `macos-universal`. Only runs on `main` save it, because GitHub
scopes caches to a ref: a tag can read `main`'s cache but not an earlier tag's. So keep
`main` building (it does on every code push) and a tag build only compiles our own crates.
The first run on a fresh repository fills the cache and is the slow one. A newer push to
`main` cancels a build still running for an older one.

Not done yet:

- **Signing.** Neither build is code-signed. Windows SmartScreen asks first (More info,
  then Run anyway). macOS says the app "cannot be checked for malicious software": right
  click the app, then Open, or run `xattr -dr com.apple.quarantine ThaanDB.app`. Signing
  and notarizing need an Apple Developer ID and a Windows code-signing certificate stored
  as repository secrets.
- **Linux** builds.
- The version shown in the app comes from `Cargo.toml`, not from the tag: keep them the same.
