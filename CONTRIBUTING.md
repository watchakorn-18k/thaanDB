# Contributing to ThaanDB

## Issues welcome, pull requests closed

**ThaanDB does not accept pull requests.** Pull requests are closed without review,
however small or useful they are.

ThaanDB is maintained by one person, who does not have the time to review outside code
line by line. A database client holds passwords and has direct access to people's data,
so code nobody has reviewed is a real risk: one hidden change could steal credentials or
ship malware to everyone who installs the next release. Taking no outside code is the
simplest way to keep every release safe.

What helps most is a good issue. Bugs and ideas you report shape what gets built next.

## Reporting a bug

Open an [issue](https://github.com/watchakorn-18k/thaanDB/issues) with:

- the ThaanDB version (or commit) and your OS
- the database and its version, for example PostgreSQL 17 or MongoDB 7
- the steps to reproduce it, what you expected and what happened instead
- a screenshot or the error message, if there is one

Leave out passwords, connection strings with credentials, hostnames and private data.
Swap them for placeholders such as `postgres://user:***@host/db`.

## Asking for a feature

Describe what you want to do and why, not only the button you want. "I want to compare
two tables' rows before a migration" helps more than "add a diff button".

You may describe how another database client behaves, in your own words or with a
screenshot of its interface. Never paste code, CSS, SQL templates or other material taken
from another client: ThaanDB is written clean-room (see
[`docs/clean-room.md`](docs/clean-room.md)), and such an issue is closed.

## Security problems

Do not post a working exploit or the details of a vulnerability in a public issue. Open
an issue that only asks for a private contact, and send the details once you have one.

## Forks

ThaanDB is licensed under the [Apache License 2.0](LICENSE). You are free to fork it,
change it and ship your own build under the terms of that licence. Changes made in a fork
do not come back into this repository.

## Code of conduct

Be respectful and constructive. Criticise code and behaviour, not people. Comments or
accounts that harass others are removed.
