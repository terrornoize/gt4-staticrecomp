# Shared save library (several PCs)

Saves > **Shared folder** > **Choose shared folder…** keeps the saves and replays of both games in a folder
that several PCs share, for example `C:\Users\<you>\OneDrive\SaveGames\gt4-staticrecomp`. The choice is stored per PC in
`settings.toml` as `[install] library_path`.

## What goes where

| In the shared folder | Stays on each PC |
|---|---|
| `games\<id>\Saves` (active save, slots, `.backup` copies) | settings, controls, wheel (`%APPDATA%\GT4Recomp\config`) |
| `games\<id>\Replays` | card backups (`games\<id>\backups`, about 8 MB each) |
| `games\<id>\in-use.json` while a PC plays | the runtime card (`<install>\data\games\<id>\runtime\card.ps2`) |

## Choosing the folder

- A game whose library in the shared folder is still empty gets a **copy** of this PC's saves and replays.
- A game that already has saves there (another PC put them there) **uses those**; this PC's library is left where it was.
- Nothing is moved or deleted. **Use this PC's folder** goes back to the local library and leaves the shared folder as it is.

So: choose the folder on the PC that has the saves first, let it sync, then choose the same folder on the other PCs.

## Safety rules

- **Stale card.** Before each Play the launcher used to harvest the runtime card left by the last run. With a shared
  library that card can be older than the library (another PC saved meanwhile). The launcher now records the SHA-256 of the
  card it wrote or harvested (`card.ps2.harvested`) and harvests only a card the game changed since then (a run whose
  harvest did not happen). This applies with or without a shared folder.
- **Folder not there** (OneDrive not signed in, drive missing): the launcher shows this PC's library but refuses Play, so
  the progress does not split between two places.
- **Another PC is playing**: `in-use.json` names the PC that started the game; it is removed after the harvest. Play on
  another PC warns once; a second Play press starts anyway (for a PC that crashed and left the file behind). Cloud sync
  takes time, so this is a warning, not a lock.

## OneDrive tips

- Right-click the folder > **Always keep on this device**, so the files are never cloud-only placeholders.
- Wait for the sync icon to turn green before playing on another PC.

## Code

`launcher/crates/gt4l-core/src/shared_library.rs` (folder check, copy, card record, in-use marker),
`paths.rs` (`UserDirs::with_shared_library`), `config.rs` (`library_path`), `gt4l-tauri/src/state.rs`
(`open_libraries`), `saves.rs` (`prepare_card`, `harvest_card`), `commands.rs` (`play`, `library_state`,
`choose_library_folder`, `reset_library_folder`), `ui/js/pages/library.js` (`mountSharedFolder`).
