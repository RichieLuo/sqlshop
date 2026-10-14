/// Server-observed state of this query document's connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueryTransactionState {
    #[default]
    Unknown,
    Idle,
    Active,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionCommand {
    Begin,
    Commit,
    Rollback,
}
