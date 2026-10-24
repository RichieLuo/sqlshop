//! Local draft persistence. SQL text (including history and saved snapshots) may itself
//! contain secrets. This format intentionally contains no result sets or credentials.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

// Older clients must reject engine-specific documents rather than interpreting
// their SQL as an unbound query for a different database engine.
pub const WORKSPACE_VERSION: u32 = 2;
pub const MAX_HISTORY_ENTRIES: usize = 200;
pub const MAX_SQL_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_WORKSPACE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryTarget {
    pub connection: Uuid,
    pub database: Option<String>,
    /// PostgreSQL search_path target; SQL Server's observed user default schema.
    /// SQL Server restores it by validation only, never ALTER USER. Older
    /// workspaces omitted this field.
    #[serde(default)]
    pub schema: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedTab {
    pub id: Uuid,
    pub name: String,
    pub sql: String,
    pub file_path: Option<PathBuf>,
    pub saved_sql: String,
    pub target: Option<QueryTarget>,
    #[serde(default)]
    pub saved_query: Option<crate::catalog::SavedQuery>,
    /// Independent engine documents retain their identity without credentials,
    /// live connections, transaction state or result sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<SavedEngineDocument>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "engine", rename_all = "snake_case")]
pub enum SavedEngineDocument {
    Sqlite {
        database_path: PathBuf,
    },
    Oracle {
        username: String,
        connect_string: String,
        schema: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub sql: String,
    pub target: QueryTarget,
    pub source: String,
    /// Unix timestamp in seconds.
    pub timestamp: i64,
    pub success: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WorkspaceLayout {
    pub navigation: bool,
    pub information: bool,
    pub information_force: bool,
    pub toolbar: bool,
    pub status: bool,
    pub focus: bool,
    pub connected_only: bool,
    pub theme: String,
}
impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            navigation: true,
            information: true,
            information_force: false,
            toolbar: true,
            status: true,
            focus: false,
            connected_only: false,
            theme: "system".into(),
        }
    }
}

/// Presentation preferences only: no row values or credentials are persisted.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct GridColumnLayout {
    pub columns: Vec<GridColumnPreference>,
    pub frozen_through: Option<ColumnIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnIdentity {
    pub name: String,
    pub occurrence: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GridColumnPreference {
    pub identity: ColumnIdentity,
    pub hidden: bool,
    pub width: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableGridLayout {
    pub connection: Uuid,
    pub database: String,
    pub schema: String,
    pub table: String,
    pub layout: GridColumnLayout,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub version: u32,
    pub tabs: Vec<SavedTab>,
    pub active_tab: usize,
    pub next_tab: usize,
    /// Oldest first; new executions are appended.
    pub history: Vec<HistoryEntry>,
    pub editor_height: Option<f32>,
    #[serde(default)]
    pub layout: Option<WorkspaceLayout>,
    #[serde(default)]
    pub table_grid_layouts: Vec<TableGridLayout>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: WORKSPACE_VERSION,
            tabs: Vec::new(),
            active_tab: 0,
            next_tab: 1,
            history: Vec::new(),
            editor_height: None,
            layout: None,
            table_grid_layouts: Vec::new(),
        }
    }
}

impl Workspace {
    pub fn push_history(&mut self, entry: HistoryEntry) {
        self.history.push(entry);
        self.trim_history();
    }

    fn trim_history(&mut self) {
        let excess = self.history.len().saturating_sub(MAX_HISTORY_ENTRIES);
        self.history.drain(..excess);
    }

    fn normalize(&mut self) {
        self.trim_history();
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        self.next_tab = self.next_tab.max(self.tabs.len().saturating_add(1));
        self.editor_height = self.editor_height.filter(|h| h.is_finite() && *h > 0.0);
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceStore {
    path: PathBuf,
}

impl WorkspaceStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            path: directory.join("workspace.json"),
        }
    }

    /// Only a missing file means a fresh workspace. Callers must disable automatic
    /// saves on any other error, so an unreadable/corrupt workspace is not replaced.
    pub fn load(&self) -> Result<Workspace> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Workspace::default());
            }
            Err(error) => return Err(error).context("Cannot open workspace"),
        };
        let bytes = read_limited(file, MAX_WORKSPACE_BYTES).context("Cannot read workspace")?;
        let mut workspace: Workspace = serde_json::from_slice(&bytes)
            .context("Workspace is not valid JSON or has invalid fields")?;
        if workspace.version == 1 {
            workspace.version = WORKSPACE_VERSION;
        }
        validate_version(&workspace)?;
        workspace.normalize();
        Ok(workspace)
    }

    pub fn save(&self, workspace: &Workspace) -> Result<()> {
        validate_version(workspace)?;
        let mut workspace = workspace.clone();
        workspace.normalize();
        let bytes = serde_json::to_vec_pretty(&workspace).context("Cannot serialize workspace")?;
        if bytes.len() > MAX_WORKSPACE_BYTES {
            bail!(
                "Workspace exceeds the {} MiB limit",
                MAX_WORKSPACE_BYTES / 1024 / 1024
            );
        }
        let directory = self
            .path
            .parent()
            .context("Workspace has no parent directory")?;
        fs::create_dir_all(directory).context("Cannot create workspace directory")?;
        atomic_write(&self.path, &bytes).context("Cannot save workspace")
    }
}

fn validate_version(workspace: &Workspace) -> Result<()> {
    if workspace.version != WORKSPACE_VERSION {
        bail!("Unsupported workspace version {}", workspace.version);
    }
    Ok(())
}

/// Read UTF-8 SQL, accepting and stripping an optional UTF-8 BOM. The bounded
/// read also handles files that grow after the metadata check.
pub fn read_sql_file(path: &Path) -> Result<String> {
    let file = File::open(path).context("Cannot open SQL file")?;
    let bytes = read_limited(file, MAX_SQL_FILE_BYTES).context("Cannot read SQL file")?;
    let text = String::from_utf8(bytes).context("SQL file must be UTF-8")?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
}

/// Persist SQL without truncating the old destination before the new contents
/// have been fully written and flushed. Failure leaves the old file intact.
pub fn write_sql_file(path: &Path, sql: &str) -> Result<()> {
    if sql.len() > MAX_SQL_FILE_BYTES {
        bail!(
            "SQL file exceeds the {} MiB limit",
            MAX_SQL_FILE_BYTES / 1024 / 1024
        );
    }
    atomic_write(path, sql.as_bytes()).context("Cannot save SQL file")
}

fn read_limited(file: File, limit: usize) -> Result<Vec<u8>> {
    if !file.metadata()?.is_file() {
        bail!("Expected a regular file");
    }
    if file.metadata()?.len() > limit as u64 {
        bail!("File exceeds the {} byte limit", limit);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bail!("File exceeds the {} byte limit", limit);
    }
    Ok(bytes)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Never follow a destination symlink or replace a special file.
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("Destination is not a regular file")
        }
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let temporary = parent.join(format!(".sqlshop-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .context("Cannot create temporary file")?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let encode = |path: &Path| -> std::io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Path contains NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    };
    let source = encode(source)?;
    let destination = encode(destination)?;
    // Same-directory move, replace existing, write through. Do not fall back to
    // removing the old file: that would destroy it if the second operation failed.
    let success = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) };
    if success == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: i64) -> HistoryEntry {
        HistoryEntry {
            sql: format!("SELECT {index}, '中文 🦀'"),
            target: QueryTarget {
                connection: Uuid::new_v4(),
                database: Some("资料库".into()),
                schema: None,
            },
            source: "SQL 1".into(),
            timestamp: index,
            success: index % 2 == 0,
        }
    }

    #[test]
    fn restores_unicode_unsaved_draft_target_and_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::new(directory.path().to_owned());
        assert_eq!(store.load().unwrap(), Workspace::default());
        let mut workspace = Workspace::default();
        workspace.tabs.push(SavedTab {
            id: Uuid::new_v4(),
            name: "未保存 🦀".into(),
            sql: "SELECT '你好'".into(),
            saved_sql: "SELECT 1".into(),
            file_path: Some(directory.path().join("中文.sql")),
            target: Some(entry(1).target),
            saved_query: None,
            document: None,
        });
        workspace.next_tab = 2;
        workspace.editor_height = Some(260.0);
        workspace.push_history(entry(1));
        store.save(&workspace).unwrap();
        assert_eq!(store.load().unwrap(), workspace);
        workspace.tabs[0].sql.push_str("; -- changed");
        store.save(&workspace).unwrap();
        assert_eq!(store.load().unwrap(), workspace);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn legacy_saved_tabs_without_engine_or_catalog_binding_still_load() {
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::new(directory.path().to_owned());
        let mut json = serde_json::to_value(Workspace::default()).unwrap();
        json["version"] = serde_json::json!(1);
        json["tabs"] = serde_json::json!([{
            "id": Uuid::new_v4(), "name": "Legacy", "sql": "SELECT '草稿'",
            "file_path": null, "saved_sql": "SELECT 1", "target": null
        }]);
        fs::write(&store.path, serde_json::to_vec(&json).unwrap()).unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.version, WORKSPACE_VERSION);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&store.path).unwrap()).unwrap()["version"],
            1
        );
        assert_eq!(restored.tabs[0].sql, "SELECT '草稿'");
        assert!(restored.tabs[0].document.is_none());
        assert!(restored.tabs[0].saved_query.is_none());
        store.save(&restored).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&store.path).unwrap()).unwrap()["version"],
            WORKSPACE_VERSION
        );
    }

    #[test]
    fn corruption_and_future_versions_are_not_silently_recovered() {
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::new(directory.path().to_owned());
        for bytes in [b"{".as_slice(), b"{}", b"\xff"] {
            fs::write(&store.path, bytes).unwrap();
            assert!(store.load().is_err());
            assert_eq!(fs::read(&store.path).unwrap(), bytes);
        }
        let workspace = Workspace {
            version: 999,
            ..Workspace::default()
        };
        fs::write(&store.path, serde_json::to_vec(&workspace).unwrap()).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&workspace).is_err());
    }

    #[test]
    fn history_is_bounded_on_append_save_and_load() {
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::new(directory.path().to_owned());
        let mut workspace = Workspace::default();
        for i in 0..250 {
            workspace.push_history(entry(i));
        }
        assert_eq!(workspace.history.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(workspace.history[0].timestamp, 50);
        workspace.history.extend((250..300).map(entry));
        store.save(&workspace).unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.history.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(restored.history[0].timestamp, 100);
        fs::write(&store.path, serde_json::to_vec(&workspace).unwrap()).unwrap();
        assert_eq!(store.load().unwrap().history, restored.history);
    }

    #[test]
    fn sql_files_are_utf8_bounded_and_safely_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("文稿.sql");
        write_sql_file(&path, "SELECT '你好 🦀';").unwrap();
        assert_eq!(read_sql_file(&path).unwrap(), "SELECT '你好 🦀';");
        write_sql_file(&path, "SELECT 2;").unwrap();
        let too_large = "x".repeat(MAX_SQL_FILE_BYTES + 1);
        assert!(write_sql_file(&path, &too_large).is_err());
        assert_eq!(read_sql_file(&path).unwrap(), "SELECT 2;");
        fs::write(&path, b"\xef\xbb\xbfSELECT 1").unwrap();
        assert_eq!(read_sql_file(&path).unwrap(), "SELECT 1");
        fs::write(&path, b"\xff").unwrap();
        assert!(read_sql_file(&path).is_err());
        fs::write(&path, too_large).unwrap();
        assert!(read_sql_file(&path).is_err());
        assert!(write_sql_file(directory.path(), "oops").is_err());
    }

    #[test]
    fn restored_layout_and_active_index_are_normalized() {
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::new(directory.path().to_owned());
        let workspace = Workspace {
            active_tab: usize::MAX,
            next_tab: 0,
            editor_height: Some(-25.0),
            ..Workspace::default()
        };
        fs::write(&store.path, serde_json::to_vec(&workspace).unwrap()).unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.active_tab, 0);
        assert_eq!(restored.next_tab, 1);
        assert_eq!(restored.editor_height, None);
    }

    #[cfg(unix)]
    #[test]
    fn saving_refuses_symlinks_without_changing_the_target() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.sql");
        let link = directory.path().join("link.sql");
        fs::write(&original, "keep me").unwrap();
        symlink(&original, &link).unwrap();
        assert!(write_sql_file(&link, "replacement").is_err());
        assert_eq!(fs::read_to_string(&original).unwrap(), "keep me");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_and_sql_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::new(directory.path().to_owned());
        store.save(&Workspace::default()).unwrap();
        assert_eq!(
            fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let path = directory.path().join("draft.sql");
        fs::write(&path, "old").unwrap();
        write_sql_file(&path, "new").unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
