# Clean-room policy

This project is an independent, open-source database manager. It aims to offer a
feature set and a layout that users of existing SQL clients find familiar. It is
**not** a copy, fork or port of any existing product's code.

## What we may use

| Source | Allowed use |
| --- | --- |
| Observed behaviour of other clients (using the app, screenshots we take ourselves, public videos) | Describe *what* a feature does in our own words in `docs/spec/` |
| Public marketing pages, user docs, changelogs | Feature lists and user-visible behaviour only |
| Official protocol and dialect docs (PostgreSQL, MySQL/MariaDB, SQLite, SQL Server TDS, Oracle, ClickHouse, ...) | Implementing drivers and SQL generation |
| Open-source crates under permissive licences (MIT / Apache-2.0 / BSD / Zlib / MPL-2.0 as a dependency) | Normal dependencies, recorded in `Cargo.toml` |
| Icon sets under CC0 / MIT / ISC that are not a database client (lucide via gpui-kit, Simple Icons for database logos) | Shipped unchanged with source, version and licence recorded next to the files (`crates/thaan-app/assets/db-icons/SOURCES.md`). Database logos are trademarks: used only to name the database a connection is for |
| General UI conventions (sidebar + tabs + editor + result grid) | Freely |

## What we must never do

1. **Never read, decompile, de-minify or unpack the source of another database client** for
   this project. This includes Electron `app.asar` bundles, JS source maps and
   "readable" shipped code, whether the product is proprietary or open source.
   Copyleft (GPL/AGPL) sources are excluded too: reading them taints our code.
2. Never use reverse-engineering tools (REA, decompilers, debuggers, Frida, etc.) on
   another database client's binaries for this project.
3. Never paste code, SQL templates, CSS, icons, logos, fonts, sounds, illustrations or
   localisation strings from another product.
4. Never use another product's name or trademark in our product name, crate names,
   window titles, icons or marketing. Comparisons in docs ("similar to …") are fine.
5. Never copy a product's exact colour palette, icon set or pixel layout. We match
   *workflow and layout conventions*, and draw our own design (see
   `docs/spec/ui.md`).

## Process

- A feature starts as a behaviour spec in `docs/spec/features.md` (what the user sees and
  does), written from the allowed sources above and citing the type of source.
- Implementation is written only from that spec plus official protocol docs.
- Whoever writes the code must not have read another client's source for that area. If
  they have, someone else implements it.
- The project takes no outside pull requests (`CONTRIBUTING.md`). Issues may describe
  another client's behaviour or show screenshots of its interface, but code, CSS or SQL
  templates pasted from another client are removed and never used.
