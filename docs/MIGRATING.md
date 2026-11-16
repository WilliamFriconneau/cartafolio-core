# Migrating from HemCP

MemCP is the name this project took when it became public; before that it was developed privately as HemCP, up to version 0.22.7. Nothing in the data changed shape because of the name: the same databases, the same keys, the same addresses (`/mcp/<connection>`). The program opens them as they are and brings them up to date by itself (a database is migrated the first time it is opened, which is why the first start after an update takes a moment, and why a copy is the first step).

## The Home Assistant app, updated in place

Keep the **slug** of the app you already have (`hemcp`): it is what carries the data folder, and the data is read from it as before. Only the name shown and the programs change.

1. **Back up.** Make a backup of the app from the Home Assistant backups page (the app is backed up cold, so the copy is consistent). Keep it until you have checked the update.
2. **Replace the app's folder**, if you run it as a local app, with the one of this release: the new programs in `bin/<arch>/` (`memcp`, `memcp-manager`, `memcp-janitor`), the new `run.sh`, `config.yaml` with the same `slug`, and a higher `version`. If you installed it from a repository, update it from the store.
3. **Update the app** from Home Assistant (a local app: reload the store, then update). Its options are kept.
4. **Check.** Open the page from the sidebar; the people and spaces are there; an assistant that was connected still works with the same key at the same address. The log of the app says which version started.

## Going back

A database that was opened by the new version has been migrated, and an older version does not open a database newer than itself. To go back, restore the backup of step 1 (which restores the data and the app together). Do not put the old programs on the migrated data.

## From a plain installation

Stop the server, copy the data directory, start the new `memcp` on the copy first (`memcp --data COPY doctor` reads it and says what it finds, and writes nothing), and switch only when it looks right. Environment variables and files that carried the old name (`HEMCP_*`, `~/.config/hemcp`) now carry the new one: rename them, and the key variable is `MEMCP_TOKEN`; the keys themselves, which start with `hcp_`, are unchanged.

## What is new in 0.23

See [CHANGELOG.md](../CHANGELOG.md): the layout of spaces, a dependency fix and the designs for what follows.
