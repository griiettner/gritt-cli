# Upgrading

## Binary

Replace the binary. Configuration files, the keychain entries, the model
cache, and the database are all forward compatible.

## Database migrations

Product migrations are additive and applied automatically the first time
a new version opens the database. Each is recorded once in
`gritt_schema_migrations`; a migration never runs twice and never rewrites
or drops existing rows. An end-to-end test opens a database created by the
first release schema, upgrades it, and checks that its sessions survive.

Inspect the resulting state:

```bash
gritt doctor
```

Opening the database is what applies pending migrations, so `doctor`
upgrades first and then reports. Its `database` section lists every known
migration as applied after that upgrade and reports whether the
`gritt-agent` memory namespace is present. To see the state before an
upgrade, run the older binary's `doctor` or read `gritt_schema_migrations`
directly. The memory namespace is owned by `gritt-agent` and is never
migrated by `gritt`.

## Rolling back

An older binary can open a newer database: it ignores tables and columns
it does not know. Sessions created by a newer version may not resume on an
older one if their continuation state changed shape; the error names the
session.

## Model cache

Cache files are per profile and self-describing. A newer version that
changes the cache shape refetches the list on the next refresh; nothing
needs deleting.

## Configuration

New config keys have defaults. A key that later becomes invalid fails
loudly at load time with the file and key named, never silently.
