# CLAUDE.md

@AGENTS.md

## Notes for Claude Code

- `AGENTS.md` (imported above) is the source of truth for goals, principles, layout and
  commands. Keep both files in sync: project rules go in `AGENTS.md`, Claude-specific notes here.
- **Clean-room is a hard rule.** Do not use the REA / craft-anything / decompiler / Frida
  tools, `npx asar extract`, or plain `cat`/`grep` on another database client's installed
  app (for example anything under `/Applications/*.app` that is a DB client) or its source
  repository. If a task asks for that, stop and point to `docs/clean-room.md`. Using such an
  app as a user (screenshots of its UI) to write a behaviour spec in `docs/spec/` is fine.
- **Build, check and test with `blaze`, not `cargo`.** Use `blaze build`, `blaze check`,
  `blaze test`, `blaze run` and `blaze fmt` (same arguments as cargo, e.g.
  `blaze test --workspace --exclude thaan-app`) from the repo root; no `.` folder argument.
  `blaze check` already runs clippy's lints, so it replaces `cargo clippy`. The commands in
  `AGENTS.md` still say `cargo`: run them as `blaze`. The installed blaze is pinned to
  `0.20261105.4` (`curl -fsSL https://ix.dev/blaze | sh -s -- --version 0.20261105.4`); do not
  run `blaze update`, since 0.20261122.0 ships no toolchain on macOS (indexable-inc/blaze#2432).
  Use `cargo` only where blaze cannot build: `thaan-driver-mongodb` (blaze can't compile its
  `macro_magic_core_macros` dependency, so `cargo check -p thaan-driver-mongodb`).
- blaze's `unused import: ColumnType::*` warnings in `thaan-driver-mysql` are false: the
  import makes the `MYSQL_TYPE_*` match arms name variants. Keep it.
- Before running the GUI, build headless crates first: `blaze test --workspace --exclude thaan-app`.
  The first `blaze build -p thaan-app` compiles GPUI and takes several minutes; run it in
  the background.
- GPUI APIs change between versions. Check the pinned version in `crates/thaan-app/Cargo.toml`
  and read examples from the crate source in `~/.cargo/registry/src/*/gpui-component-*`
  rather than relying on memory.
- **UI changes start with `/unslop`.** Before designing, changing or fixing anything in the
  GUI (`crates/thaan-app`: layout, spacing, colours, theme tokens, icons, copy, motion),
  invoke the `unslop` skill first and follow it, then make the change. This applies
  every time, including small tweaks. Keep `docs/spec/ui.md` in sync with what it decides.
- When you finish a feature, update its status in `docs/spec/features.md`.
- Seeing the GUI on macOS: the screenshot tools only find bundled apps. Run
  `scripts/bundle-macos.sh` (add `--release` for a release build): it builds `thaan-app` with cargo
  (blaze cannot build its mongodb dependency yet) and
  writes `target/ThaanDB.app` with bundle id `dev.thaandb.app` and the app icon from
  `crates/thaan-app/assets/macos/AppIcon.icns`. It removes the old binary before copying
  (overwriting it in place gets the process killed). Do not `codesign` the bundle (the window
  then stops being visible to the tools). Launch with
  `THAAN_HOME=target/thaan-home nohup target/ThaanDB.app/Contents/MacOS/thaan-app &`.
- The app icon and the title-bar mark come from `docs/brand` (see its README). Regenerate
  `AppIcon.icns` from `docs/brand/logo/thaandb-macos-icon*.svg` with `iconutil` if the logo changes.
- Use `THAAN_HOME=target/thaan-home` for manual testing so the user's real store is untouched.
