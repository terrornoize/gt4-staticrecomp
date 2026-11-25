//! A save library folder shared by several PCs (`[install] library_path`), for example a OneDrive folder.
//!
//! Only `Saves` and `Replays` of each game live there (`<shared>\games\<id>\...`, see
//! [`crate::paths::UserDirs::with_shared_library`]); settings, controls, the card backups and the runtime card stay on each
//! PC. Three things make that safe:
//!
//! - [`adopt`]: when the folder is chosen, a game whose shared library is still empty gets a copy of this PC's library;
//!   a game that already has saves there (another PC put them there) uses those, and this PC's library is left as it
//!   is. Nothing is ever moved or deleted.
//! - [`record_card`] / [`card_is_recorded`]: the launcher remembers the fingerprint of the runtime card it last wrote or
//!   harvested. Before a start it harvests the card only when the card changed since then (the game ran and the
//!   harvest after it did not happen). Without that, a PC whose card is older than the shared library (another PC saved
//!   meanwhile) would harvest its stale card over the newer save.
//! - [`InUse`]: a small file in the shared game folder names the PC that is playing, so another PC can warn before it
//!   starts the same save. File locks do not travel through cloud sync; this file does.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::UserDirs;

/// Why a folder cannot be the shared library folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryPathError {
    NotAbsolute,
    NotFound,
    NotAFolder,
    /// The launcher's data folder or a folder inside it (replaced by updates, holds the runtime card).
    InsideData,
    /// This PC's own user data folder or a folder inside it (the library would nest into itself).
    InsideUserData,
}

impl LibraryPathError {
    /// The id of the text in the locale files (`rust.<id>`).
    pub fn message_id(self) -> &'static str {
        match self {
            LibraryPathError::NotAbsolute => "library.invalid_not_absolute",
            LibraryPathError::NotFound => "library.invalid_not_found",
            LibraryPathError::NotAFolder => "library.invalid_not_folder",
            LibraryPathError::InsideData => "library.invalid_data",
            LibraryPathError::InsideUserData => "library.invalid_user_data",
        }
    }
}

/// `path` tidied (a trailing separator and `.` dropped), or why it cannot be the shared library folder. Nothing is
/// created or written.
pub fn check_library_path(path: &Path, data_root: &Path, user_root: &Path) -> Result<PathBuf, LibraryPathError> {
    if !path.is_absolute() {
        return Err(LibraryPathError::NotAbsolute);
    }
    let tidy: PathBuf = path.components().collect();
    let metadata = fs::metadata(&tidy).map_err(|_| LibraryPathError::NotFound)?;
    if !metadata.is_dir() {
        return Err(LibraryPathError::NotAFolder);
    }
    let chosen = resolved(&tidy);
    if is_inside(&chosen, &resolved(data_root)) {
        return Err(LibraryPathError::InsideData);
    }
    if is_inside(&chosen, &resolved(user_root)) {
        return Err(LibraryPathError::InsideUserData);
    }
    Ok(tidy)
}

fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `path` is `folder` or below it (case does not count on Windows).
fn is_inside(path: &Path, folder: &Path) -> bool {
    let lower = |p: &Path| -> Vec<String> { p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect() };
    let folder = lower(folder);
    !folder.is_empty() && lower(path).starts_with(&folder)
}

/// Whether `folder` holds at least one file anywhere below it. An empty `Active` folder is not a save.
pub fn holds_files(folder: &Path) -> bool {
    let Ok(entries) = fs::read_dir(folder) else {
        return false;
    };
    entries.flatten().any(|entry| match entry.file_type() {
        Ok(kind) if kind.is_dir() => holds_files(&entry.path()),
        Ok(_) => true,
        Err(_) => false,
    })
}

/// What [`adopt`] did for one game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Adoption {
    /// This PC's saves and replays were copied into the shared folder.
    Copied,
    /// The shared folder already held saves or replays of this game; they are used, this PC's are left as they are.
    UsedShared,
    /// Neither held anything.
    Empty,
}

/// Gets the shared folder ready for one game: when `shared` holds no file of the game yet, this PC's `Saves` and
/// `Replays` (from `local`) are copied there; otherwise the shared ones are used. `local` and `shared` name the same
/// game. Nothing is moved or deleted; a copy goes to a staging folder first and is renamed into place, so a failed copy
/// leaves no half library behind.
pub fn adopt(local: &UserDirs, shared: &UserDirs) -> io::Result<Adoption> {
    let pairs = [(local.saves(), shared.saves()), (local.replays(), shared.replays())];
    if pairs.iter().any(|(_, to)| holds_files(to)) {
        return Ok(Adoption::UsedShared);
    }
    if !pairs.iter().any(|(from, _)| holds_files(from)) {
        return Ok(Adoption::Empty);
    }
    for (from, to) in &pairs {
        if !holds_files(from) {
            continue;
        }
        let staging = to.with_file_name(format!(
            "{}.copying",
            to.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
        ));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        copy_tree(from, &staging)?;
        // `to` holds no file (checked above): an empty folder tree left by an earlier open is replaced.
        if to.exists() {
            fs::remove_dir_all(to)?;
        }
        fs::rename(&staging, to)?;
    }
    Ok(Adoption::Copied)
}

/// Copies the folder tree `from` to `to` (made new).
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// The file beside the runtime card that holds the fingerprint of the card the launcher last wrote or harvested.
pub fn card_record_path(runtime_card: &Path) -> PathBuf {
    let name = runtime_card.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "card".to_string());
    runtime_card.with_file_name(format!("{name}.harvested"))
}

fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Remembers `bytes` as the card the launcher wrote or harvested last. A failed write only means the next start
/// harvests the card once more, as before.
pub fn record_card(runtime_card: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(card_record_path(runtime_card), fingerprint(bytes))
}

/// Whether `bytes` is the card recorded by [`record_card`]: the game did not change it since the launcher wrote or
/// harvested it, so there is nothing on it to harvest.
pub fn card_is_recorded(runtime_card: &Path, bytes: &[u8]) -> bool {
    fs::read_to_string(card_record_path(runtime_card)).is_ok_and(|text| text.trim() == fingerprint(bytes))
}

/// Name of the marker file in `<shared>\games\<id>`.
pub const IN_USE_FILE: &str = "in-use.json";

/// The PC that is playing a game from the shared folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InUse {
    /// The Windows computer name.
    pub computer: String,
    /// When the game started, seconds since 1970 (UTC).
    pub since_unix: u64,
}

/// The marker file of a game's shared library, or `None` without a shared folder.
pub fn in_use_path(user: &UserDirs) -> Option<PathBuf> {
    user.shared_game_library().map(|folder| folder.join(IN_USE_FILE))
}

/// The marker in the shared folder of `user`'s game, when there is one that can be read.
pub fn read_in_use(user: &UserDirs) -> Option<InUse> {
    let text = fs::read_to_string(in_use_path(user)?).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the marker for `computer` (no shared folder: nothing to do).
pub fn mark_in_use(user: &UserDirs, computer: &str, since_unix: u64) -> io::Result<()> {
    let Some(path) = in_use_path(user) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let marker = InUse { computer: computer.to_string(), since_unix };
    let text = serde_json::to_string_pretty(&marker).map_err(io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, &path)
}

/// Removes the marker when it is `computer`'s own; one written by another PC meanwhile is left alone.
pub fn clear_in_use(user: &UserDirs, computer: &str) -> io::Result<()> {
    let Some(path) = in_use_path(user) else {
        return Ok(());
    };
    match read_in_use(user) {
        Some(marker) if !marker.computer.eq_ignore_ascii_case(computer) => Ok(()),
        _ => match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

/// The marker of another PC (not `computer`), when there is one: the game may be running there.
pub fn used_elsewhere(user: &UserDirs, computer: &str) -> Option<InUse> {
    read_in_use(user).filter(|marker| !marker.computer.eq_ignore_ascii_case(computer))
}

/// This PC's name for the marker (`COMPUTERNAME`), or `this PC`.
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME").ok().filter(|name| !name.trim().is_empty()).unwrap_or_else(|| "this PC".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::{GT4, SPEC2};
    use crate::paths::UserRootSource;

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gt4l-shared-library-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn saves_and_replays_go_to_the_shared_folder_and_the_rest_stays_on_this_pc() {
        let local = UserDirs::new(r"C:\Users\someone\AppData\Roaming\GT4Recomp", UserRootSource::AppData);
        let shared_root = Path::new(r"C:\Users\someone\OneDrive\SaveGames\gt4-staticrecomp");
        let shared = local.with_shared_library(shared_root).for_game(&SPEC2);
        assert_eq!(shared.saves(), shared_root.join("games").join("spec2").join("Saves"));
        assert_eq!(shared.replays(), shared_root.join("games").join("spec2").join("Replays"));
        let local_spec2 = local.for_game(&SPEC2);
        assert_eq!(shared.card_backups(), local_spec2.card_backups(), "the 8 MB card backups stay on this PC");
        assert_eq!(shared.config_file(), local.config_file(), "settings stay on this PC");
        assert_eq!(shared.controls_file(), local.controls_file(), "controls stay on this PC");
        assert_eq!(shared.local().saves(), local_spec2.saves());
        assert_eq!(shared.shared_library(), Some(shared_root));
    }

    #[test]
    fn a_shared_folder_ignores_the_flat_gt4_place() {
        let mut local = UserDirs::new(r"C:\data", UserRootSource::AppData);
        local.set_flat_gt4(true);
        assert_eq!(local.saves(), Path::new(r"C:\data\Saves"));
        let shared = local.with_shared_library(Path::new(r"E:\cloud"));
        assert_eq!(shared.for_game(&GT4).saves(), Path::new(r"E:\cloud\games\gt4\Saves"));
    }

    #[test]
    fn the_folder_check_refuses_the_data_and_user_folders() {
        let base = scratch("check");
        let data = base.join("data");
        let user = base.join("user");
        let cloud = base.join("cloud");
        let file = base.join("file.txt");
        for folder in [&data, &user, &cloud] {
            fs::create_dir_all(folder).unwrap();
        }
        fs::create_dir_all(data.join("games")).unwrap();
        fs::create_dir_all(user.join("games")).unwrap();
        write(&file, "x");
        assert_eq!(check_library_path(Path::new("cloud"), &data, &user), Err(LibraryPathError::NotAbsolute));
        assert_eq!(check_library_path(&base.join("nope"), &data, &user), Err(LibraryPathError::NotFound));
        assert_eq!(check_library_path(&file, &data, &user), Err(LibraryPathError::NotAFolder));
        assert_eq!(check_library_path(&data, &data, &user), Err(LibraryPathError::InsideData));
        assert_eq!(check_library_path(&data.join("games"), &data, &user), Err(LibraryPathError::InsideData));
        assert_eq!(check_library_path(&user.join("games"), &data, &user), Err(LibraryPathError::InsideUserData));
        assert_eq!(check_library_path(&cloud, &data, &user), Ok(cloud.clone()));
        let mut with_separator = cloud.clone().into_os_string();
        with_separator.push("\\");
        assert_eq!(check_library_path(Path::new(&with_separator), &data, &user), Ok(cloud.clone()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_first_pc_copies_its_library_and_the_second_uses_the_shared_one() {
        let base = scratch("adopt");
        let cloud = base.join("cloud");
        fs::create_dir_all(&cloud).unwrap();

        // First PC: a save and a replay, the shared folder is empty.
        let first = UserDirs::new(base.join("first"), UserRootSource::AppData).for_game(&SPEC2);
        write(&first.saves().join("Active").join("BASCUS-97436GAMEDATA").join("_item.hex"), "save of the first PC");
        write(&first.replays().join("BASCUS-97436R0001").join("_item.hex"), "replay");
        // An earlier open left an empty tree in the shared folder: it is not a library.
        fs::create_dir_all(first.with_shared_library(&cloud).saves().join("Active")).unwrap();
        let shared_first = first.with_shared_library(&cloud);
        assert_eq!(adopt(&first, &shared_first).unwrap(), Adoption::Copied);
        assert_eq!(
            fs::read_to_string(shared_first.saves().join("Active").join("BASCUS-97436GAMEDATA").join("_item.hex")).unwrap(),
            "save of the first PC"
        );
        assert!(shared_first.replays().join("BASCUS-97436R0001").join("_item.hex").is_file());
        assert!(first.saves().join("Active").join("BASCUS-97436GAMEDATA").join("_item.hex").is_file(), "nothing is moved");
        assert!(!shared_first.saves().with_file_name("Saves.copying").exists(), "no staging folder is left");

        // Second PC with an older save of its own: the shared save wins, its own stays where it is.
        let second = UserDirs::new(base.join("second"), UserRootSource::AppData).for_game(&SPEC2);
        write(&second.saves().join("Active").join("BASCUS-97436GAMEDATA").join("_item.hex"), "older save");
        let shared_second = second.with_shared_library(&cloud);
        assert_eq!(adopt(&second, &shared_second).unwrap(), Adoption::UsedShared);
        assert_eq!(
            fs::read_to_string(shared_second.saves().join("Active").join("BASCUS-97436GAMEDATA").join("_item.hex")).unwrap(),
            "save of the first PC"
        );
        assert_eq!(fs::read_to_string(second.saves().join("Active").join("BASCUS-97436GAMEDATA").join("_item.hex")).unwrap(), "older save");

        // A new PC with nothing, and a game nobody played: nothing to do.
        let fresh = UserDirs::new(base.join("fresh"), UserRootSource::AppData).for_game(&GT4);
        assert_eq!(adopt(&fresh, &fresh.with_shared_library(&cloud)).unwrap(), Adoption::Empty);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_recorded_card_is_known_until_the_game_changes_it() {
        let base = scratch("record");
        let card = base.join("runtime").join("card.ps2");
        fs::create_dir_all(card.parent().unwrap()).unwrap();
        assert!(!card_is_recorded(&card, b"card"), "no record yet: harvest as before");
        record_card(&card, b"card").unwrap();
        assert_eq!(card_record_path(&card), base.join("runtime").join("card.ps2.harvested"));
        assert!(card_is_recorded(&card, b"card"));
        assert!(!card_is_recorded(&card, b"card the game wrote"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_in_use_marker_names_the_other_pc_and_only_its_owner_clears_it() {
        let base = scratch("in-use");
        let local = UserDirs::new(base.join("user"), UserRootSource::AppData).for_game(&SPEC2);
        assert_eq!(in_use_path(&local), None, "no shared folder, no marker");
        mark_in_use(&local, "DESKTOP", 1).unwrap();
        assert!(!base.join("user").join("games").join("spec2").join(IN_USE_FILE).exists());

        let shared = local.with_shared_library(&base.join("cloud"));
        assert_eq!(used_elsewhere(&shared, "DESKTOP"), None);
        mark_in_use(&shared, "DESKTOP", 1_764_000_000).unwrap();
        assert_eq!(read_in_use(&shared), Some(InUse { computer: "DESKTOP".into(), since_unix: 1_764_000_000 }));
        assert_eq!(used_elsewhere(&shared, "desktop"), None, "names compare without case");
        assert_eq!(used_elsewhere(&shared, "LEGION-GO").map(|marker| marker.computer), Some("DESKTOP".to_string()));
        clear_in_use(&shared, "LEGION-GO").unwrap();
        assert!(read_in_use(&shared).is_some(), "another PC's marker is left alone");
        clear_in_use(&shared, "DESKTOP").unwrap();
        assert_eq!(read_in_use(&shared), None);
        clear_in_use(&shared, "DESKTOP").unwrap();
        let _ = fs::remove_dir_all(&base);
    }
}
