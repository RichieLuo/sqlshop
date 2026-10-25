use super::*;

#[test]
fn configured_databases_include_empty_dbs_and_optional_key_counts_without_selecting() {
    let mock = Mock::start(vec![
        ping(), // A legacy database=5 profile must not send SELECT during handshake.
        exact(&[b"SELECT", b"7"], b"+OK\r\n".to_vec()),
        exact(&[b"CONFIG", b"GET", b"databases"], array(vec![bulk(b"databases"), bulk(b"32")])),
        exact(&[b"INFO", b"keyspace"], bulk(b"# Keyspace\r\ndb0:keys=3,expires=1,avg_ttl=100\r\ndb7:keys=42,expires=0,avg_ttl=0\r\ndb31:keys=18446744073709551615,expires=0\r\n")),
        exact(&[b"SCAN", b"0", b"MATCH", b"*", b"COUNT", b"200"], scan(0, &[])),
    ]);
    let worker = RedisWorker::start(|| {});
    let mut profile = mock.profile.clone();
    profile.database = 5;
    let json = serde_json::to_string(&profile).unwrap();
    let legacy: RedisProfile = serde_json::from_str(&json).unwrap();
    assert_eq!(legacy.database, 5);
    assert_eq!(RedisProfile::default().database, 0);
    connect(&worker, legacy, Credentials::default());
    run(&worker, RedisRequest::SelectDb(7));
    worker.send(12, RedisRequest::Databases).unwrap();
    let result = response(&worker);
    assert_eq!(result.database, 7);
    assert!(result.connected);
    let RedisData::Databases {
        databases,
        complete,
        notice,
    } = result.result.unwrap()
    else {
        panic!()
    };
    assert!(complete);
    assert!(notice.is_none(), "{notice:?}");
    assert_eq!(databases.len(), 32);
    assert!(
        databases
            .iter()
            .enumerate()
            .all(|(index, db)| db.index == index as u32)
    );
    assert_eq!(databases[0].keys, Some(3));
    assert_eq!(databases[7].keys, Some(42));
    assert_eq!(databases[31].keys, Some(u64::MAX));
    assert_eq!(databases[5].keys, Some(0));
    worker
        .send(
            13,
            RedisRequest::Scan {
                cursor: 0,
                pattern: "*".into(),
            },
        )
        .unwrap();
    assert_eq!(response(&worker).database, 7);
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn denied_config_falls_back_to_active_and_current_db_without_inventing_empty_dbs() {
    for config_error in [
        b"-NOPERM not allowed private-auth-secret\r\n".to_vec(),
        b"-ERR unknown command 'CONFIG' private-auth-secret\r\n".to_vec(),
    ] {
        let mock = Mock::start(vec![
            ping(),
            exact(&[b"SELECT", b"5"], b"+OK\r\n".to_vec()),
            exact(&[b"CONFIG", b"GET", b"databases"], config_error),
            exact(
                &[b"INFO", b"keyspace"],
                bulk(b"# Keyspace\r\ndb2:keys=8,expires=0\r\ndb31:keys=2,expires=0\r\n"),
            ),
        ]);
        let worker = RedisWorker::start(|| {});
        connect(&worker, mock.profile.clone(), Credentials::default());
        run(&worker, RedisRequest::SelectDb(5));
        worker.send(2, RedisRequest::Databases).unwrap();
        let result = response(&worker);
        assert_eq!(result.database, 5);
        assert!(result.connected);
        let RedisData::Databases {
            databases,
            complete,
            notice,
        } = result.result.unwrap()
        else {
            panic!()
        };
        assert!(!complete);
        assert_eq!(
            databases,
            vec![
                RedisDatabase {
                    index: 2,
                    keys: Some(8)
                },
                RedisDatabase {
                    index: 5,
                    keys: Some(0)
                },
                RedisDatabase {
                    index: 31,
                    keys: Some(2)
                },
            ]
        );
        let notice = notice.unwrap();
        assert!(notice.contains("无法枚举空 DB"));
        assert!(!notice.contains("private-auth-secret"));
        run(&worker, RedisRequest::Disconnect);
        mock.finish();
    }
}

#[test]
fn database_metadata_permissions_and_invalid_info_never_leak_secrets_or_claim_key_counts() {
    let mock = Mock::start(vec![
        ping(),
        exact(
            &[b"CONFIG", b"GET", b"databases"],
            array(vec![bulk(b"databases"), bulk(b"3")]),
        ),
        exact(
            &[b"INFO", b"keyspace"],
            b"-NOPERM private-auth-secret\r\n".to_vec(),
        ),
        exact(
            &[b"CONFIG", b"GET", b"databases"],
            b"-NOPERM private-auth-secret\r\n".to_vec(),
        ),
        exact(
            &[b"INFO", b"keyspace"],
            b"-NOPERM private-auth-secret\r\n".to_vec(),
        ),
        exact(
            &[b"CONFIG", b"GET", b"databases"],
            array(vec![bulk(b"databases"), bulk(b"3")]),
        ),
        exact(
            &[b"INFO", b"keyspace"],
            bulk(b"db1:keys=secret-auth-value\r\nmalformed-private-auth-secret\r\n"),
        ),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    for (expected_count, expected_complete) in [(3, true), (1, false), (3, true)] {
        let RedisData::Databases {
            databases,
            complete,
            notice,
        } = run(&worker, RedisRequest::Databases)
        else {
            panic!()
        };
        assert_eq!(databases.len(), expected_count);
        assert_eq!(complete, expected_complete);
        assert!(databases.iter().all(|db| db.keys.is_none()));
        assert!(!notice.unwrap().contains("private-auth-secret"));
    }
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn malformed_config_count_is_bounded_validated_and_falls_back_safely() {
    let invalid = [
        array(vec![]),
        array(vec![bulk(b"databases")]),
        array(vec![bulk(b"databases"), bulk(b"2"), bulk(b"extra")]),
        array(vec![bulk(b"requirepass"), bulk(b"private-auth-secret")]),
        array(vec![bulk(b"databases"), bulk(b"0")]),
        array(vec![bulk(b"databases"), bulk(b"-1")]),
        array(vec![bulk(b"databases"), bulk(b"+32")]),
        array(vec![bulk(b"databases"), bulk(b"32 ")]),
        array(vec![bulk(b"databases"), bulk(b"18446744073709551616")]),
        array(vec![bulk(b"databases"), bulk(&vec![b'9'; 4096])]),
        array(vec![bulk(b"databases"), bulk(&[0xff])]),
        array(vec![bulk(b"databases"), integer(32)]),
        bulk(b"private-auth-secret"),
    ];
    for config in invalid {
        let mock = Mock::start(vec![
            ping(),
            exact(&[b"CONFIG", b"GET", b"databases"], config),
            exact(&[b"INFO", b"keyspace"], bulk(b"db9:keys=1,expires=0\r\n")),
        ]);
        let worker = RedisWorker::start(|| {});
        connect(&worker, mock.profile.clone(), Credentials::default());
        let RedisData::Databases {
            databases,
            complete,
            notice,
        } = run(&worker, RedisRequest::Databases)
        else {
            panic!()
        };
        assert!(!complete);
        assert_eq!(
            databases,
            vec![
                RedisDatabase {
                    index: 0,
                    keys: Some(0)
                },
                RedisDatabase {
                    index: 9,
                    keys: Some(1)
                }
            ]
        );
        let notice = notice.unwrap();
        assert!(notice.contains("无法枚举空 DB"));
        assert!(!notice.contains("private-auth-secret"));
        run(&worker, RedisRequest::Disconnect);
        mock.finish();
    }
}

#[test]
fn large_database_counts_are_truncated_without_scanning_or_changing_current_db() {
    for count in ["4096", "4097", "18446744073709551615"] {
        let mock = Mock::start(vec![
            ping(),
            exact(&[b"SELECT", b"5000"], b"+OK\r\n".to_vec()),
            exact(
                &[b"CONFIG", b"GET", b"databases"],
                array(vec![bulk(b"databases"), bulk(count.as_bytes())]),
            ),
            exact(
                &[b"INFO", b"keyspace"],
                bulk(b"# Keyspace\r\ndb5000:keys=9,expires=0\r\n"),
            ),
        ]);
        let worker = RedisWorker::start(|| {});
        connect(&worker, mock.profile.clone(), Credentials::default());
        run(&worker, RedisRequest::SelectDb(5000));
        worker.send(3, RedisRequest::Databases).unwrap();
        let result = response(&worker);
        assert_eq!(result.database, 5000);
        let RedisData::Databases {
            databases,
            complete,
            notice,
        } = result.result.unwrap()
        else {
            panic!()
        };
        assert!(!complete);
        assert_eq!(databases.len(), 4096);
        assert_eq!(
            databases.last(),
            Some(&RedisDatabase {
                index: 5000,
                keys: Some(9)
            })
        );
        assert!(notice.unwrap().contains("truncated"));
        run(&worker, RedisRequest::Disconnect);
        mock.finish();
    }
}

#[test]
fn oversized_config_response_closes_connection_without_allocating_or_leaking_diagnostics() {
    let mock = Mock::start(vec![
        ping(),
        exact(&[b"SELECT", b"3"], b"+OK\r\n".to_vec()),
        exact(
            &[b"CONFIG", b"GET", b"databases"],
            b"*999999999\r\n".to_vec(),
        ),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    run(&worker, RedisRequest::SelectDb(3));
    worker.send(4, RedisRequest::Databases).unwrap();
    let failed = response(&worker);
    assert_eq!(failed.database, 3);
    assert!(!failed.connected);
    assert!(failed.result.unwrap_err().contains("安全上限"));
    mock.finish();
}

#[test]
fn failed_selection_preserves_current_db_and_string_cas_cache_while_enumeration_is_read_only() {
    let mock = Mock::start(vec![
        ping(),
        step("TYPE", b"+string\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("STRLEN", integer(3)),
        step("GETRANGE", bulk(b"old")),
        exact(
            &[b"SELECT", b"999"],
            b"-ERR DB index is out of range\r\n".to_vec(),
        ),
        exact(
            &[b"CONFIG", b"GET", b"databases"],
            array(vec![bulk(b"databases"), bulk(b"32")]),
        ),
        exact(&[b"INFO", b"keyspace"], bulk(b"db0:keys=1,expires=0\r\n")),
        step("TYPE", b"+string\r\n".to_vec()),
        step("EVAL", integer(1)),
        exact(&[b"SELECT", b"1"], b"+OK\r\n".to_vec()),
        step("TYPE", b"+string\r\n".to_vec()),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    run(&worker, RedisRequest::ReadKey { key: b"k".to_vec() });
    worker.send(4, RedisRequest::SelectDb(999)).unwrap();
    let failed = response(&worker);
    assert_eq!(failed.database, 0);
    assert!(failed.result.is_err());
    run(&worker, RedisRequest::Databases);
    let save = || RedisRequest::Mutate {
        key: b"k".to_vec(),
        change: RedisMutation::SetString {
            value: b"new".to_vec(),
            expected: b"old".to_vec(),
        },
    };
    assert!(matches!(run(&worker, save()), RedisData::Changed { .. }));
    run(&worker, RedisRequest::SelectDb(1));
    worker.send(5, save()).unwrap();
    assert!(response(&worker).result.unwrap_err().contains("完整读取"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}
