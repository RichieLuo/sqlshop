//! Catalogs and typed changes. Passwords are transient account request secrets;
//! catalogs, previews and persisted state never contain them.
use crate::model::Table;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalKind {
    User,
    Login,
    Role,
}

impl PrincipalKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "用户",
            Self::Login => "服务器登录",
            Self::Role => "角色",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPrincipal {
    pub name: String,
    pub host: Option<String>,
    pub kind: PrincipalKind,
    /// PostgreSQL: LOGIN attribute; MySQL: not account-locked; SQL Server
    /// logins: not disabled. Database users/roles may have no such flag.
    pub enabled: Option<bool>,
    pub default_schema: Option<String>,
    /// Non-secret catalog identity for stale-account checks.
    pub identity: Option<String>,
}

impl AccessPrincipal {
    pub fn display_name(&self) -> String {
        match &self.host {
            Some(host) => format!("{}@{host}", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SecurityCatalog {
    pub principals: Vec<AccessPrincipal>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TablePrivilege {
    Select,
    Insert,
    Update,
    Delete,
    References,
}

impl TablePrivilege {
    pub const ALL: [Self; 5] = [
        Self::Select,
        Self::Insert,
        Self::Update,
        Self::Delete,
        Self::References,
    ];

    pub fn sql(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::References => "REFERENCES",
        }
    }
}

#[derive(Clone, Debug)]
pub enum SecurityChange {
    CreateRole {
        name: String,
    },
    DropRole {
        role: AccessPrincipal,
    },
    GrantRole {
        role: AccessPrincipal,
        member: AccessPrincipal,
    },
    RevokeRole {
        role: AccessPrincipal,
        member: AccessPrincipal,
    },
    GrantTablePrivilege {
        principal: AccessPrincipal,
        table: Table,
        privilege: TablePrivilege,
    },
    RevokeTablePrivilege {
        principal: AccessPrincipal,
        table: Table,
        privilege: TablePrivilege,
    },
}

/// Transient account secret. Deliberately has neither Debug nor serde.
#[derive(Clone)]
pub struct AccountPassword(String);

impl AccountPassword {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

/// These typed requests are never written to history or workspace storage.
#[derive(Clone)]
pub enum AccountChange {
    CreateUser {
        name: String,
        host: Option<String>,
        password: AccountPassword,
    },
    ChangePassword {
        principal: AccessPrincipal,
        password: AccountPassword,
        old_password: Option<AccountPassword>,
    },
    SetLogin {
        principal: AccessPrincipal,
        enabled: bool,
    },
    DropUser {
        principal: AccessPrincipal,
    },
    CreateLogin {
        name: String,
        password: AccountPassword,
    },
    CreateDatabaseUser {
        name: String,
        login: AccessPrincipal,
        default_schema: Option<String>,
    },
}
