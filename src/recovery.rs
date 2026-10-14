//! Read-only evidence for an uncertain table submission. These states describe
//! values observed now; they do not prove which transaction wrote those values.

use crate::model::{CellUpdate, InsertValue, Table};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationState {
    Original,
    Intended,
    Conflict,
    Missing,
    Unknown,
}

impl VerificationState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "当前仍为原值",
            Self::Intended => "当前符合暂存值",
            Self::Conflict => "当前值存在冲突",
            Self::Missing => "原键与目标键均未找到",
            Self::Unknown => "无法判断",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServerRowEvidence {
    /// Bound key values used for this lookup, without rebasing the local draft.
    pub lookup_key: Vec<Option<String>>,
    pub columns: Vec<String>,
    /// At most two rows, so an unexpected non-unique match remains visible.
    pub rows: Vec<Vec<Option<String>>>,
    pub matches_original: bool,
    pub matches_intended: bool,
}

#[derive(Clone, Debug)]
pub struct RowVerification {
    pub primary_keys: Vec<String>,
    pub original_key: Vec<Option<String>>,
    pub intended_key: Vec<Option<String>>,
    pub changes: Vec<CellUpdate>,
    pub state: VerificationState,
    pub reason: String,
    pub evidence: Vec<ServerRowEvidence>,
}

#[derive(Clone, Debug)]
pub struct InsertVerification {
    pub draft_index: usize,
    pub values: Vec<InsertValue>,
    pub state: VerificationState,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct DraftVerification {
    pub database: String,
    pub table: Table,
    pub checked_at: String,
    pub rows: Vec<RowVerification>,
    pub insert_columns: Vec<String>,
    pub inserts: Vec<InsertVerification>,
    pub note: String,
}

impl DraftVerification {
    pub fn count(&self, state: VerificationState) -> usize {
        self.rows.iter().filter(|row| row.state == state).count()
            + self.inserts.iter().filter(|row| row.state == state).count()
    }
}
