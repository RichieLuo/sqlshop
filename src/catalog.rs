//! Durable organization, independent of connection credentials and workspace drafts.
use crate::workspace::{QueryTarget, atomic_write};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::PathBuf,
};
use uuid::Uuid;

const VERSION: u32 = 1;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionGroup {
    pub id: Uuid,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub id: Uuid,
    pub name: String,
    pub sql: String,
    pub target: Option<QueryTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    pub groups: Vec<ConnectionGroup>,
    pub memberships: HashMap<Uuid, Uuid>,
    pub queries: Vec<SavedQuery>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: VERSION,
            groups: vec![],
            memberships: HashMap::new(),
            queries: vec![],
        }
    }
}

fn checked_name(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "名称不能为空");
    ensure!(
        name.chars().count() <= 120 && !name.chars().any(char::is_control),
        "名称不能包含控制字符，最多 120 个字符"
    );
    Ok(name.to_owned())
}

impl Catalog {
    pub fn save_group(&mut self, id: Option<Uuid>, name: &str) -> Result<Uuid> {
        let name = checked_name(name)?;
        ensure!(
            !self
                .groups
                .iter()
                .any(|g| Some(g.id) != id && g.name.to_lowercase() == name.to_lowercase()),
            "已有同名连接分组"
        );
        if let Some(id) = id {
            self.groups
                .iter_mut()
                .find(|g| g.id == id)
                .context("连接分组不存在")?
                .name = name;
            Ok(id)
        } else {
            let id = Uuid::new_v4();
            self.groups.push(ConnectionGroup { id, name });
            Ok(id)
        }
    }

    /// Removing a group never removes connection profiles or saved queries.
    pub fn delete_group(&mut self, id: Uuid) {
        self.groups.retain(|g| g.id != id);
        self.memberships.retain(|_, group| *group != id);
    }

    pub fn move_connection(&mut self, connection: Uuid, group: Option<Uuid>) -> Result<()> {
        if let Some(group) = group {
            ensure!(self.groups.iter().any(|g| g.id == group), "连接分组不存在");
            self.memberships.insert(connection, group);
        } else {
            self.memberships.remove(&connection);
        }
        Ok(())
    }

    /// New queries never overwrite by name. Updates require their stable ID.
    pub fn save_query(
        &mut self,
        id: Option<Uuid>,
        name: &str,
        sql: String,
        target: Option<QueryTarget>,
    ) -> Result<Uuid> {
        let name = checked_name(name)?;
        ensure!(
            !self
                .queries
                .iter()
                .any(|q| Some(q.id) != id && q.name.to_lowercase() == name.to_lowercase()),
            "已有同名查询，请使用不同名称，或在查询库中明确更新原查询"
        );
        if let Some(id) = id {
            let query = self
                .queries
                .iter_mut()
                .find(|q| q.id == id)
                .context("已保存查询不存在")?;
            *query = SavedQuery {
                id,
                name,
                sql,
                target,
            };
            Ok(id)
        } else {
            let id = Uuid::new_v4();
            self.queries.push(SavedQuery {
                id,
                name,
                sql,
                target,
            });
            Ok(id)
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == VERSION,
            "不支持的查询库版本 {}",
            self.version
        );
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for group in &self.groups {
            ensure!(
                checked_name(&group.name)? == group.name
                    && ids.insert(group.id)
                    && names.insert(group.name.to_lowercase()),
                "连接分组数据无效或重复"
            );
        }
        ensure!(
            self.memberships.values().all(|id| ids.contains(id)),
            "连接分组引用无效"
        );
        ids.clear();
        names.clear();
        for query in &self.queries {
            ensure!(
                checked_name(&query.name)? == query.name
                    && ids.insert(query.id)
                    && names.insert(query.name.to_lowercase()),
                "已保存查询数据无效或重复"
            );
        }
        Ok(())
    }
}

pub struct CatalogStore {
    path: PathBuf,
}

impl CatalogStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            path: directory.join("catalog.json"),
        }
    }

    /// Only NotFound is a new catalog. Callers must disable writes after any error.
    pub fn load(&self) -> Result<Catalog> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Catalog::default());
            }
            Err(error) => return Err(error).context("无法读取查询库"),
        };
        ensure!(file.metadata()?.is_file(), "查询库不是普通文件");
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_BYTES, "查询库超过 64 MiB 限制");
        let catalog: Catalog =
            serde_json::from_slice(&bytes).context("查询库损坏，原文件未修改")?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn save(&self, catalog: &Catalog) -> Result<()> {
        catalog.validate()?;
        // Refuse to replace a file that became corrupt after startup as well.
        self.load()?;
        let bytes = serde_json::to_vec_pretty(catalog)?;
        ensure!(bytes.len() <= MAX_BYTES, "查询库超过 64 MiB 限制");
        fs::create_dir_all(self.path.parent().context("查询库目录不可用")?)?;
        atomic_write(&self.path, &bytes).context("保存查询库失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_move_rename_delete_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(dir.path().into());
        let mut catalog = store.load().unwrap();
        let connection = Uuid::new_v4();
        let group = catalog.save_group(None, "开发环境").unwrap();
        catalog.move_connection(connection, Some(group)).unwrap();
        catalog.save_group(Some(group), "测试环境").unwrap();
        store.save(&catalog).unwrap();
        assert_eq!(store.load().unwrap(), catalog);
        catalog.delete_group(group);
        assert!(catalog.memberships.is_empty());
        store.save(&catalog).unwrap();
        assert_eq!(store.load().unwrap(), catalog);
        assert!(catalog.move_connection(connection, Some(group)).is_err());
    }

    #[test]
    fn named_queries_preserve_sql_target_and_reject_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(dir.path().into());
        let mut catalog = Catalog::default();
        let target = Some(QueryTarget {
            schema: None,
            connection: Uuid::new_v4(),
            database: Some("生产库".into()),
        });
        let id = catalog
            .save_query(None, "每日查询", "SELECT '中文';".into(), target.clone())
            .unwrap();
        assert!(
            catalog
                .save_query(None, " 每日查询 ", "oops".into(), None)
                .is_err()
        );
        assert!(catalog.save_group(None, " \n ").is_err());
        store.save(&catalog).unwrap();
        let mut restored = store.load().unwrap();
        assert_eq!(restored, catalog);
        restored
            .save_query(Some(id), "改名", "SELECT 2".into(), target.clone())
            .unwrap();
        store.save(&restored).unwrap();
        assert_eq!(store.load().unwrap().queries[0].target, target);
        restored.queries.retain(|q| q.id != id);
        store.save(&restored).unwrap();
        assert!(store.load().unwrap().queries.is_empty());
    }

    #[test]
    fn corrupt_future_and_invalid_catalogs_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(dir.path().into());
        for bytes in [
            b"{".to_vec(),
            b"{}".to_vec(),
            serde_json::to_vec(&Catalog {
                version: 99,
                ..Catalog::default()
            })
            .unwrap(),
            serde_json::to_vec(&Catalog {
                memberships: HashMap::from([(Uuid::new_v4(), Uuid::new_v4())]),
                ..Catalog::default()
            })
            .unwrap(),
        ] {
            fs::write(&store.path, &bytes).unwrap();
            assert!(store.load().is_err());
            assert!(store.save(&Catalog::default()).is_err());
            assert_eq!(fs::read(&store.path).unwrap(), bytes);
        }
    }
}
