use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::{Column, Row, TypeInfo, ValueRef, mysql::MySqlRow, postgres::PgRow};

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::from("0x");
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

macro_rules! decode {
    ($row:expr, $index:expr, $($ty:ty),+ $(,)?) => {
        $(if let Ok(value) = $row.try_get::<$ty, _>($index) { return Some(value.to_string()); })+
    };
}

pub fn mysql(row: &MySqlRow) -> Vec<Option<String>> {
    (0..row.len())
        .map(|index| {
            if row.try_get_raw(index).is_ok_and(|value| value.is_null()) {
                return None;
            }
            decode!(
                row,
                index,
                String,
                i64,
                u64,
                i32,
                u32,
                i16,
                u16,
                i8,
                u8,
                f64,
                f32,
                rust_decimal::Decimal,
                NaiveDateTime,
                NaiveDate,
                NaiveTime,
                serde_json::Value
            );
            if let Ok(bytes) = row.try_get::<Vec<u8>, _>(index) {
                return Some(hex(&bytes));
            }
            Some(format!(
                "[暂不支持的类型: {}]",
                row.columns()[index].type_info().name()
            ))
        })
        .collect()
}

pub fn postgres(row: &PgRow) -> Vec<Option<String>> {
    (0..row.len())
        .map(|index| {
            if row.try_get_raw(index).is_ok_and(|value| value.is_null()) {
                return None;
            }
            decode!(
                row,
                index,
                String,
                bool,
                i64,
                i32,
                i16,
                f64,
                f32,
                rust_decimal::Decimal,
                DateTime<Utc>,
                NaiveDateTime,
                NaiveDate,
                NaiveTime,
                uuid::Uuid,
                serde_json::Value
            );
            if let Ok(bytes) = row.try_get::<Vec<u8>, _>(index) {
                return Some(hex(&bytes));
            }
            if let Ok(array) = row.try_get::<Vec<Option<String>>, _>(index) {
                return Some(format!("{array:?}"));
            }
            if let Ok(array) = row.try_get::<Vec<Option<i64>>, _>(index) {
                return Some(format!("{array:?}"));
            }
            if let Ok(array) = row.try_get::<Vec<Option<i32>>, _>(index) {
                return Some(format!("{array:?}"));
            }
            Some(format!(
                "[暂不支持的类型: {}]",
                row.columns()[index].type_info().name()
            ))
        })
        .collect()
}

pub fn sqlserver(row: tiberius::Row) -> Vec<Option<String>> {
    use tiberius::{ColumnData as D, FromSql};
    row.into_iter()
        .map(|value| match value {
            D::U8(v) => v.map(|v| v.to_string()),
            D::I16(v) => v.map(|v| v.to_string()),
            D::I32(v) => v.map(|v| v.to_string()),
            D::I64(v) => v.map(|v| v.to_string()),
            D::F32(v) => v.map(|v| v.to_string()),
            D::F64(v) => v.map(|v| v.to_string()),
            D::Bit(v) => v.map(|v| v.to_string()),
            D::String(v) => v.map(|v| v.into_owned()),
            D::Guid(v) => v.map(|v| v.to_string()),
            D::Binary(v) => v.map(|v| hex(&v)),
            D::Numeric(v) => v.map(|v| v.to_string()),
            D::Xml(v) => v.map(|v| v.to_string()),
            D::Date(_) => NaiveDate::from_sql(&value)
                .ok()
                .flatten()
                .map(|v| v.to_string()),
            D::Time(_) => NaiveTime::from_sql(&value)
                .ok()
                .flatten()
                .map(|v| v.to_string()),
            D::DateTimeOffset(_) => chrono::DateTime::<chrono::FixedOffset>::from_sql(&value)
                .ok()
                .flatten()
                .map(|v| v.to_string()),
            _ => NaiveDateTime::from_sql(&value)
                .ok()
                .flatten()
                .map(|v| v.to_string()),
        })
        .collect()
}
