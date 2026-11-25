# Database logos

| File | Source | Version | Licence |
| --- | --- | --- | --- |
| `postgresql.svg`, `mysql.svg`, `mariadb.svg`, `sqlite.svg`, `mongodb.svg`, `redis.svg`, `mqtt.svg` | [Simple Icons](https://simpleicons.org) (`simple-icons` npm package, `icons/<slug>.svg`) | 16.38.1 | CC0 1.0 |

Fetched on 2026-11-25 (`redis.svg`, `mqtt.svg` on 2026-11-26) from `https://cdn.jsdelivr.net/npm/simple-icons@16.38.1/icons/`. The
files are unchanged (24×24 viewBox, single path).

## Trademarks

The logos are trademarks of their owners: PostgreSQL (PostgreSQL Community Association),
MySQL (Oracle), MariaDB (MariaDB Foundation / MariaDB plc), SQLite (Hipp, Wyrick & Company),
MongoDB (MongoDB, Inc.), Redis (Redis Ltd.), MQTT (the MQTT logo of mqtt.org / OASIS). ThaanDB uses them only to show which database a connection or
driver is for. It does not use them as its own mark, does not alter their shapes, and
implies no endorsement.

## Adding a logo

Take it from Simple Icons (or another CC0 / MIT icon set that is not a database client),
add a row above, add the path to `OWN` in `crates/thaan-app/src/assets.rs` and to
`db_logo_path`. Never copy logos from another database client (`docs/clean-room.md`).
