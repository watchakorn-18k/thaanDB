# MongoDB UI spec

Behaviour spec for working with MongoDB in ThaanDB. Written in our own words from:

- the MongoDB manual (documents, BSON types, query/projection/sort, shell syntax);
- using desktop MongoDB GUIs as an ordinary user against a local test database
  (observed 2026-11-25: connection dialog, sidebar tree, result views, paging).

Nothing here comes from another product's code, binaries, resources or styling (see
`docs/clean-room.md`). The layout follows conventions common to document-database tools;
the visual design (colours, icons, spacing, wording) is ThaanDB's own.

## What users expect (observed conventions)

1. **Sidebar tree**: connection → databases → per-database folders *Collections (n)*,
   *Views (n)*, *System (n)* → collections, each expandable to its indexes.
2. **Result views**: the same result can be switched between
   - a **table** (one row per document, one column per top-level field),
   - a **tree** (one row per document; expand to see fields; columns *Key / Value / Type*),
   - **JSON** text (pretty-printed shell syntax, read-only, foldable).
3. **Nested values are summarised** in the table: an object shows `{ 3 fields }`, an
   array `[ 2 elements ]`; expanding happens in the tree view.
4. **Type is always visible**: each value is coloured by BSON type and the tree view has a
   type column (*ObjectId, String, Int32, Int64, Double, Decimal128, Bool, Date, Object,
   Array, Null, Binary, Regex*).
5. **Paging**: page size choice and a "Documents a to b" counter, with first / previous /
   next / last.
6. **Collection stats** (count, data size, storage size, indexes) are one hover or click
   away from the collection in the sidebar.
7. Connection settings group server, authentication, TLS, SSH and advanced options, and
   can be imported from / exported to a connection string.

## ThaanDB behaviour

| # | Behaviour | Status |
| --- | --- | --- |
| M1 | Result panel for MongoDB has a segmented switch **Table · Tree · JSON**; the choice is remembered per tab | gui |
| M2 | Table cells for objects/arrays show `{ n fields }` / `[ n items ]`; scalars as before | gui |
| M3 | Tree view: one row per document labelled `#n  _id: …`; disclosure arrows expand objects and arrays to any depth; columns Field · Value · Type | gui |
| M4 | JSON view: pretty-printed shell syntax (`ObjectId("…")`, `ISODate("…")`, `NumberDecimal("…")`), two-space indent, read-only, selectable/copyable | gui |
| M5 | Values are coloured by type using theme tokens (string, number, bool, date/id, object, array, null) — our palette, defined in `theme.rs` | gui |
| M6 | Collection view query bar has three inputs: **Filter**, **Projection**, **Sort** (each a query document); Enter in any of them re-runs | gui |
| M7 | Collection footer reads "Documents a–b of n" | gui |
| M8 | Hover/click stats for a collection in the sidebar | todo |
| M9 | Icon toolbar above the collection view: insert · view · edit · delete (tooltips; write actions disabled on read-only connections), with the selected `_id` on the right | gui |
| M9a | **Insert** dialog: shell-syntax editor, Validate, Format, Wrap lines, Insert, and "Insert and add another" (inserts, clears, stays open, counts) | gui |
| M9b | **View** dialog: the full document re-read from the server (never the projected row), read-only, with Edit to switch in place | gui |
| M9c | **Edit** dialog: same tools as Insert; Update runs `replaceOne({ _id }, doc)`. Documents with values that cannot round-trip as text (binary other than UUID, timestamps, non-finite doubles) open read-only | gui |
| M9d | **Delete** dialog: scope *Selected document* · *Current filter* · *All documents*, the query shown and editable, a live match count, and a button that says how many go ("Delete 4 documents"); disabled at 0 matches | gui |
| M9g | With a document selected (table row or cell), the status bar adds **Edit** (`pencil`; **View** with `eye` on a read-only connection) and **Delete** (`trash`) after Insert. Edit opens the M9c dialog. Delete asks first ("Delete this document?", its `_id` and collection, cannot be undone), then runs `deleteOne({ _id })` in the background and reloads the page. Hidden when nothing is selected | gui |
| M9e | Errors from the server stay in the dialog as the server's own sentence; the text is kept for fixing | gui |
| M9f | Edit a single value in place in the tree or table | todo |
| M10 | Connection dialog sections (TLS, SSH, auth mechanism) | todo |

## Data model

The engine gives the GUI structured documents, not only flattened text:
`ResultSet::documents: Option<Vec<Doc>>` where `Doc` is a typed tree
(`thaan_core::doc::Doc`). SQL drivers leave it `None`. The table grid keeps using
`ResultSet::rows`; the tree and JSON views read `documents`.
