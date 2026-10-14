//! Source-bound diagnostic/permission documents. Connection credentials never
//! enter document state; account form secrets are transient and never persisted.
use crate::{
    diagnostics::DiagnosticKind,
    model::{Engine, Profile, QueryOutput},
    security::{AccessPrincipal, AccountChange, SecurityCatalog, SecurityChange, TablePrivilege},
    workspace::QueryTarget,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvancedKind {
    Diagnostics(DiagnosticKind),
    Plan,
    Security,
}

impl AdvancedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Diagnostics(kind) => kind.label(),
            Self::Plan => "执行计划",
            Self::Security => "用户与权限",
        }
    }
}

#[derive(Clone)]
pub struct AdvancedSource {
    pub profile: Profile,
    pub target: QueryTarget,
}

impl AdvancedSource {
    pub fn new(profile: Profile, target: QueryTarget) -> Self {
        Self { profile, target }
    }

    pub fn same_binding(&self, other: &Self) -> bool {
        let (Ok(profile), Ok(other_profile)) = (
            serde_json::to_value(&self.profile),
            serde_json::to_value(&other.profile),
        ) else {
            return false;
        };
        self.target == other.target && profile == other_profile
    }
}

#[derive(Clone)]
pub enum AdvancedOperation {
    RefreshDiagnostics(DiagnosticKind),
    Explain(String),
    LoadPrincipals,
    LoadGrants(AccessPrincipal),
    ApplySecurity {
        change: SecurityChange,
        preview_sql: String,
    },
    ApplyAccount {
        change: AccountChange,
        preview_sql: String,
        preview_source: Box<AdvancedSource>,
    },
}

impl AdvancedOperation {
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::ApplySecurity { .. } | Self::ApplyAccount { .. })
    }
}

pub enum AdvancedPending {
    Read,
    Explain(String),
    Mutation,
}
impl AdvancedPending {
    fn is_mutation(&self) -> bool {
        matches!(self, Self::Mutation)
    }
}

pub enum AdvancedResult {
    Data(QueryOutput),
    Principals(SecurityCatalog),
    Grants {
        principal: AccessPrincipal,
        output: QueryOutput,
    },
    SecurityChanged {
        catalog: Option<SecurityCatalog>,
        warning: Option<String>,
        sql: String,
    },
}

pub struct AdvancedFailure {
    pub message: String,
    /// No acknowledgement for a submitted mutation. Never automatically replay.
    pub outcome_unknown: bool,
}

pub struct AdvancedResponse {
    pub document: Uuid,
    pub generation: u64,
    pub source: AdvancedSource,
    pub result: Result<AdvancedResult, AdvancedFailure>,
}

#[derive(Clone)]
pub struct SecurityPreview {
    pub change: SecurityChange,
    pub sql: String,
}

pub struct AccountPreview {
    pub change: AccountChange,
    pub sql: String,
    pub source: AdvancedSource,
}

pub struct AdvancedDocument {
    pub id: Uuid,
    pub source: AdvancedSource,
    pub kind: AdvancedKind,
    pub generation: u64,
    pub busy: bool,
    pub pending: Option<AdvancedPending>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub outcome_unknown: bool,
    pub output: Option<QueryOutput>,
    pub catalog: Option<SecurityCatalog>,
    pub selected_principal: Option<AccessPrincipal>,
    pub sql: String,
    pub last_plan_sql: Option<String>,
    pub result_index: usize,
    pub search: String,
    pub role_name: String,
    pub role: Option<AccessPrincipal>,
    pub member: Option<AccessPrincipal>,
    pub table_schema: String,
    pub table_name: String,
    pub privilege: TablePrivilege,
    pub security_form: usize,
    pub plan_tree: bool,
    pub preview: Option<SecurityPreview>,
    pub account_preview: Option<AccountPreview>,
    pub account_form: usize,
    pub account_name: String,
    pub account_host: String,
    pub account_password: String,
    pub account_password_confirm: String,
    pub account_old_password: String,
    pub account_login: Option<AccessPrincipal>,
    pub account_schema: String,
}

impl AdvancedDocument {
    pub fn new(source: AdvancedSource, kind: AdvancedKind) -> Self {
        let table_schema =
            source
                .target
                .schema
                .clone()
                .unwrap_or_else(|| match source.profile.engine {
                    Engine::MySql => source.target.database.clone().unwrap_or_default(),
                    Engine::Postgres => "public".into(),
                    Engine::SqlServer => "dbo".into(),
                });
        Self {
            id: Uuid::new_v4(),
            source,
            kind,
            generation: 0,
            busy: false,
            pending: None,
            error: None,
            notice: None,
            outcome_unknown: false,
            output: None,
            catalog: None,
            selected_principal: None,
            sql: "SELECT 1;".into(),
            last_plan_sql: None,
            result_index: 0,
            search: String::new(),
            role_name: String::new(),
            role: None,
            member: None,
            table_schema,
            table_name: String::new(),
            privilege: TablePrivilege::Select,
            security_form: 0,
            plan_tree: true,
            preview: None,
            account_preview: None,
            account_form: 0,
            account_name: String::new(),
            account_host: "%".into(),
            account_password: String::new(),
            account_password_confirm: String::new(),
            account_old_password: String::new(),
            account_login: None,
            account_schema: "dbo".into(),
        }
    }

    pub fn refresh_operation(&self) -> AdvancedOperation {
        match self.kind {
            AdvancedKind::Diagnostics(kind) => AdvancedOperation::RefreshDiagnostics(kind),
            AdvancedKind::Plan => AdvancedOperation::Explain(self.sql.clone()),
            AdvancedKind::Security => AdvancedOperation::LoadPrincipals,
        }
    }

    /// Returns None for overlapping actions, retaining the in-flight generation.
    pub fn begin(&mut self, operation: AdvancedOperation) -> Option<u64> {
        if self.busy {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.busy = true;
        self.error = None;
        self.notice = None;
        self.pending = Some(match operation {
            AdvancedOperation::Explain(sql) => AdvancedPending::Explain(sql),
            operation if operation.is_mutation() => AdvancedPending::Mutation,
            _ => AdvancedPending::Read,
        });
        Some(self.generation)
    }

    pub fn can_close(&self) -> bool {
        !self.busy
            || self
                .pending
                .as_ref()
                .is_none_or(|operation| !operation.is_mutation())
    }

    pub fn matches_response(&self, response: &AdvancedResponse) -> bool {
        self.id == response.document
            && self.generation == response.generation
            && self.source.same_binding(&response.source)
    }

    pub fn apply_response(&mut self, response: AdvancedResponse) -> bool {
        if !self.matches_response(&response) {
            return false;
        }
        self.busy = false;
        let pending = self.pending.take();
        match response.result {
            Ok(AdvancedResult::Data(output)) => {
                if let Some(AdvancedPending::Explain(sql)) = pending {
                    self.last_plan_sql = Some(sql);
                }
                self.output = Some(output);
                self.result_index = 0;
                self.error = None;
            }
            Ok(AdvancedResult::Principals(catalog)) => {
                self.replace_catalog(catalog);
                self.error = None;
            }
            Ok(AdvancedResult::Grants { principal, output }) => {
                self.selected_principal = Some(principal);
                self.output = Some(output);
                self.result_index = 0;
                self.error = None;
            }
            Ok(AdvancedResult::SecurityChanged {
                catalog,
                warning,
                sql: _,
            }) => {
                self.preview = None;
                self.account_preview = None;
                self.error = None;
                self.output = None;
                self.outcome_unknown = false;
                self.notice = Some(warning.map_or_else(
                    || "账号或权限变更已完成。".into(),
                    |warning| format!("账号或权限变更已完成；目录刷新失败：{warning}"),
                ));
                if let Some(catalog) = catalog {
                    self.replace_catalog(catalog);
                }
            }
            Err(failure) => {
                self.error = Some(failure.message);
                if failure.outcome_unknown {
                    self.outcome_unknown = true;
                    self.preview = None;
                    self.account_preview = None;
                }
            }
        }
        true
    }

    fn replace_catalog(&mut self, catalog: SecurityCatalog) {
        if let Some(selected) = self.selected_principal.as_ref() {
            let replacement = catalog
                .principals
                .iter()
                .find(|principal| {
                    principal.name == selected.name
                        && principal.host == selected.host
                        && (principal.kind == selected.kind
                            || (self.source.profile.engine == Engine::Postgres
                                && principal.identity == selected.identity))
                })
                .cloned();
            if replacement.is_none() {
                self.output = None;
            }
            self.selected_principal = replacement;
        }
        self.catalog = Some(catalog);
    }
}
