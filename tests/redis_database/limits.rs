use super::*;

#[test]
fn console_rejects_state_blocking_admin_scripts_and_unbounded_reads() {
    let worker = RedisWorker::start(|| {});
    for command in [
        "AUTH secret",
        "HELLO 3",
        "SELECT 1",
        "MULTI",
        "EXEC",
        "DISCARD",
        "WATCH k",
        "UNWATCH",
        "SUBSCRIBE x",
        "PSUBSCRIBE *",
        "SSUBSCRIBE x",
        "MONITOR",
        "QUIT",
        "RESET",
        "CLIENT REPLY OFF",
        "BLPOP k 1",
        "BRPOP k 1",
        "BLMOVE a b LEFT RIGHT 0",
        "WAIT 1 1",
        "XREAD BLOCK 0 STREAMS k $",
        "FLUSHALL",
        "FLUSHDB",
        "CONFIG GET *",
        "CONFIG GET databases",
        "INFO keyspace",
        "INFO",
        "ACL LIST",
        "DEBUG OBJECT k",
        "EVAL return 1 0",
        "EVALSHA abc 0",
        "FUNCTION LIST",
        "KEYS *",
        "HGETALL k",
        "SMEMBERS k",
        "DUMP k",
        "SCAN 0 COUNT 999999",
        "LRANGE k 0 -1",
        "ZRANGE k 0 999999",
        "GETRANGE k 0 99999999",
        "SET k v GET",
        "RENAME a b",
        "EXPIRE k 0",
        "ZADD k NaN a",
    ] {
        assert!(
            worker
                .send(
                    0,
                    RedisRequest::Command {
                        args: parse_command(command).unwrap(),
                        confirmed: true
                    }
                )
                .is_err(),
            "{command}"
        );
    }
    assert!(
        worker
            .send(
                0,
                RedisRequest::Command {
                    args: parse_command("SET k v").unwrap(),
                    confirmed: false
                }
            )
            .is_err()
    );
    assert!(
        worker
            .send(
                0,
                RedisRequest::Create {
                    key: b"x".to_vec(),
                    kind: RedisKind::Stream,
                    value: vec![],
                    field: vec![],
                    score: 0.0
                }
            )
            .is_err()
    );
    assert!(
        worker
            .send(
                0,
                RedisRequest::ReadKey {
                    key: vec![0; MAX_VALUE_BYTES + 1]
                }
            )
            .is_err()
    );
    let mut profile = RedisProfile {
        tls: true,
        ..Default::default()
    };
    profile.ssh.enabled = true;
    assert!(
        profile
            .validate()
            .unwrap_err()
            .to_string()
            .contains("TLS + SSH")
    );
}

#[test]
fn reads_use_bounded_commands_and_limit_collection_previews() {
    let items: Vec<Vec<u8>> = (0..250)
        .map(|index| bulk(index.to_string().as_bytes()))
        .collect();
    let mut hash_items = Vec::new();
    for index in 0..250 {
        hash_items.push(bulk(index.to_string().as_bytes()));
        hash_items.push(bulk(b"value"));
    }
    let mock = Mock::start(vec![
        ping(),
        step("TYPE", b"+hash\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("HLEN", integer(250)),
        exact(
            &[b"HSCAN", b"h", b"0", b"COUNT", b"200"],
            array(vec![bulk(b"0"), array(hash_items)]),
        ),
        step("TYPE", b"+list\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("LLEN", integer(300)),
        exact(
            &[b"LRANGE", b"l", b"0", b"199"],
            array(items[..200].to_vec()),
        ),
        step("TYPE", b"+set\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("SCARD", integer(250)),
        exact(
            &[b"SSCAN", b"s", b"0", b"COUNT", b"200"],
            array(vec![bulk(b"0"), array(items)]),
        ),
        step("TYPE", b"+zset\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("ZCARD", integer(2)),
        exact(
            &[b"ZRANGE", b"z", b"0", b"199", b"WITHSCORES"],
            array(vec![
                bulk(b"a\xff"),
                bulk(b"1.25"),
                bulk(b"b\0"),
                bulk(b"-2"),
            ]),
        ),
        step("TYPE", b"+stream\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("XLEN", integer(10000)),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    for name in [b"h", b"l", b"s"] {
        let RedisData::Key(key) = run(&worker, RedisRequest::ReadKey { key: name.to_vec() }) else {
            panic!()
        };
        assert!(key.truncated);
        let count = match key.value {
            RedisValue::Hash(items) => items.len(),
            RedisValue::List(items) | RedisValue::Set(items) => items.len(),
            _ => panic!(),
        };
        assert_eq!(count, MAX_COLLECTION_ITEMS);
    }
    let RedisData::Key(key) = run(&worker, RedisRequest::ReadKey { key: b"z".to_vec() }) else {
        panic!()
    };
    assert!(!key.truncated);
    assert!(
        matches!(key.value, RedisValue::ZSet(items) if items == vec![(b"a\xff".to_vec(), 1.25), (b"b\0".to_vec(), -2.0)])
    );
    let RedisData::Key(key) = run(
        &worker,
        RedisRequest::ReadKey {
            key: b"stream".to_vec(),
        },
    ) else {
        panic!()
    };
    assert!(matches!(key.value, RedisValue::Unsupported(_)));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn collection_budget_never_exposes_partial_hash_or_list_values() {
    let oversized = vec![b'x'; MAX_VALUE_BYTES];
    let hash_reply = array(vec![
        bulk(b"0"),
        array(vec![
            bulk(b"first"),
            bulk(b"complete\0\xff"),
            bulk(b"too-large"),
            bulk(&oversized),
            bulk(b"later"),
            bulk(b"must-not-be-returned"),
        ]),
    ]);
    let list_reply = array(vec![
        bulk(b"complete\0\xff"),
        bulk(&oversized),
        bulk(b"later"),
    ]);
    let mock = Mock::start(vec![
        ping(),
        step("TYPE", b"+hash\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("HLEN", integer(3)),
        step("HSCAN", hash_reply),
        step("TYPE", b"+list\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("LLEN", integer(3)),
        step("LRANGE", list_reply),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let RedisData::Key(hash) = run(
        &worker,
        RedisRequest::ReadKey {
            key: b"hash".to_vec(),
        },
    ) else {
        panic!()
    };
    assert!(hash.truncated);
    assert!(matches!(hash.value, RedisValue::Hash(items)
        if items == vec![(b"first".to_vec(), b"complete\0\xff".to_vec())]));
    let RedisData::Key(list) = run(
        &worker,
        RedisRequest::ReadKey {
            key: b"list".to_vec(),
        },
    ) else {
        panic!()
    };
    assert!(list.truncated);
    assert!(
        matches!(list.value, RedisValue::List(items) if items == vec![b"complete\0\xff".to_vec()])
    );
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn oversized_response_is_rejected_before_redis_allocation_and_closes_connection() {
    for packet in [
        b"$999999999\r\n".to_vec(),
        b"*999999999\r\n".to_vec(),
        vec![b'+'; 5000],
    ] {
        let mock = Mock::start(vec![ping(), step("GET", packet)]);
        let worker = RedisWorker::start(|| {});
        connect(&worker, mock.profile.clone(), Credentials::default());
        worker
            .send(
                2,
                RedisRequest::Command {
                    args: parse_command("GET huge").unwrap(),
                    confirmed: false,
                },
            )
            .unwrap();
        let failed = response(&worker);
        assert!(!failed.connected);
        assert!(failed.result.is_err());
        worker
            .send(3, RedisRequest::ReadKey { key: b"x".to_vec() })
            .unwrap();
        assert!(
            response(&worker)
                .result
                .unwrap_err()
                .contains("不会自动重连")
        );
        mock.finish();
    }
}
