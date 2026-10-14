//! Read-only database objects and catalog-supplied summary metadata.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    Function,
    Procedure,
    Trigger,
    Sequence,
}

impl ObjectKind {
    pub const ALL: [Self; 7] = [
        Self::Table,
        Self::View,
        Self::MaterializedView,
        Self::Function,
        Self::Procedure,
        Self::Trigger,
        Self::Sequence,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Table => "表",
            Self::View => "视图",
            Self::MaterializedView => "物化视图",
            Self::Function => "函数",
            Self::Procedure => "过程",
            Self::Trigger => "触发器",
            Self::Sequence => "序列",
        }
    }

    pub fn is_relation(self) -> bool {
        matches!(self, Self::Table | Self::View | Self::MaterializedView)
    }

    pub fn is_routine(self) -> bool {
        matches!(self, Self::Function | Self::Procedure)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogObject {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    pub owner: Option<String>,
    pub comment: Option<String>,
    /// Catalog estimate; never a full-table COUNT.
    pub estimated_rows: Option<u64>,
    /// Catalog-reported size, which may be an estimate and may be unavailable.
    pub size_bytes: Option<u64>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub engine: Option<String>,
    /// PostgreSQL identity arguments distinguish overloaded routines.
    pub signature: Option<String>,
    /// Parent relation name disambiguates PostgreSQL triggers.
    pub parent: Option<String>,
}

impl CatalogObject {
    pub fn display_name(&self) -> String {
        match &self.signature {
            Some(signature) => format!("{}({signature})", self.name),
            None => self.name.clone(),
        }
    }

    pub fn as_table(&self) -> Option<crate::model::Table> {
        self.kind.is_relation().then(|| crate::model::Table {
            schema: self.schema.clone(),
            name: self.name.clone(),
            kind: match self.kind {
                ObjectKind::Table => "BASE TABLE",
                ObjectKind::MaterializedView => "MATERIALIZED VIEW",
                _ => "VIEW",
            }
            .into(),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ObjectCatalog {
    pub objects: Vec<CatalogObject>,
    pub warnings: Vec<String>,
    /// Server errors prevented discovering these categories; zero is unknown.
    pub unavailable_kinds: Vec<ObjectKind>,
    pub truncated: bool,
}
