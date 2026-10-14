//! Read-only server diagnostics. Statistics and plan costs are server estimates.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    Activity,
    Locks,
    SlowQueries,
}

impl DiagnosticKind {
    pub const ALL: [Self; 3] = [Self::Activity, Self::Locks, Self::SlowQueries];

    pub fn label(self) -> &'static str {
        match self {
            Self::Activity => "会话与正在执行的查询",
            Self::Locks => "锁与等待",
            Self::SlowQueries => "查询统计",
        }
    }
}
