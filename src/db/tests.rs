use super::sqlserver_type_name;
use tiberius::ColumnType;

#[test]
fn sqlserver_headers_use_sql_names_not_wire_variant_names() {
    for (wire, sql) in [
        (ColumnType::Int4, "int"),
        (ColumnType::Int8, "bigint"),
        (ColumnType::Bitn, "bit"),
        (ColumnType::NVarchar, "nvarchar"),
        (ColumnType::BigVarBin, "varbinary"),
        (ColumnType::Guid, "uniqueidentifier"),
        (ColumnType::Decimaln, "decimal"),
        (ColumnType::Datetime2, "datetime2"),
    ] {
        assert_eq!(sqlserver_type_name(wire), sql);
    }
    assert_eq!(
        sqlserver_type_name(ColumnType::Datetimen),
        "datetime / smalldatetime (TDS Datetimen)"
    );
    assert!(sqlserver_type_name(ColumnType::Intn).contains("unknown"));
    assert!(sqlserver_type_name(ColumnType::Floatn).contains("unknown"));
    assert!(sqlserver_type_name(ColumnType::Udt).contains("user-defined"));
}
