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
- Before running the GUI, build headless crates first: `cargo test --workspace --exclude thaan-app`.
  The first `cargo build -p thaan-app` compiles GPUI and takes several minutes; run it in
  the background.
- GPUI APIs change between versions. Check the pinned version in `crates/thaan-app/Cargo.toml`
  and read examples from the crate source in `~/.cargo/registry/src/*/gpui-component-*`
  rather than relying on memory.
- **UI changes start with `/unslop`.** Before designing, changing or fixing anything in the
  GUI (`crates/thaan-app`: layout, spacing, colours, theme tokens, icons, copy, motion),
  invoke the `unslop` skill first and follow it, then make the change. This applies
  every time, including small tweaks. Keep `docs/spec/ui.md` in sync with what it decides.
- When you finish a feature, update its status in `docs/spec/features.md`.
- Seeing the GUI on macOS: the screenshot tools only find bundled apps. Wrap the binary in
  `target/ThaanDB.app` (`Contents/MacOS/thaan-app` + an `Info.plist` with bundle id
  `dev.thaandb.app`). After a rebuild, `rm` the old binary before copying (overwriting it in
  place gets the process killed), do not `codesign` the bundle (the window then stops
  being visible to the tools), and launch with
  `THAAN_HOME=target/thaan-home nohup target/ThaanDB.app/Contents/MacOS/thaan-app &`.
- Use `THAAN_HOME=target/thaan-home` for manual testing so the user's real store is untouched.
