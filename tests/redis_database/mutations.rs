use super::*;

#[test]
fn string_save_is_cas_preserves_ttl_and_refuses_stale_or_truncated_values() {
    let key = b"bin\0\xff";
    let script_check = |expected: &'static [u8], response: Vec<u8>| -> Step {
        Box::new(move |args| {
            assert_eq!(args[0], b"EVAL");
            let script = std::str::from_utf8(&args[1]).unwrap();
            assert!(script.contains("'TYPE'"));
            assert!(script.contains("'STRLEN'"));
            assert!(script.contains("'GET'"));
            assert!(script.contains("'KEEPTTL'"));
            assert_eq!(args[2], b"1");
            assert_eq!(args[3], b"bin\0\xff");
            assert_eq!(args[4], b"string");
            assert_eq!(args[5], b"STRING");
            assert_eq!(args[7], expected);
            response
        })
    };
    let mut steps = vec![
        ping(),
        step("TYPE", b"+string\r\n".to_vec()),
        step("PTTL", integer(5000)),
        step("STRLEN", integer(3)),
        exact(&[b"GETRANGE", key, b"0", b"1048575"], bulk(b"a\0b")),
        step("TYPE", b"+string\r\n".to_vec()),
        script_check(b"a\0b", integer(1)),
        step("TYPE", b"+string\r\n".to_vec()),
        step("PTTL", integer(4900)),
        step("STRLEN", integer(3)),
        step("GETRANGE", bulk(b"new")),
        step("TYPE", b"+string\r\n".to_vec()),
        script_check(
            b"new",
            b"-ERR String changed; reload before saving\r\n".to_vec(),
        ),
        step("TYPE", b"+string\r\n".to_vec()),
        step("PTTL", integer(-1)),
        step("STRLEN", integer(MAX_VALUE_BYTES as i64 + 50)),
        step("GETRANGE", bulk(&vec![b'x'; MAX_VALUE_BYTES])),
        step("TYPE", b"+string\r\n".to_vec()),
    ];
    let mock = Mock::start(std::mem::take(&mut steps));
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    assert!(
        matches!(run(&worker, RedisRequest::ReadKey { key: key.to_vec() }), RedisData::Key(key) if key.ttl_ms == 5000 && !key.truncated)
    );
    assert!(matches!(
        run(
            &worker,
            RedisRequest::Mutate {
                key: key.to_vec(),
                change: RedisMutation::SetString {
                    value: b"new".to_vec(),
                    expected: b"a\0b".to_vec()
                }
            }
        ),
        RedisData::Changed { deleted: false, .. }
    ));
    run(&worker, RedisRequest::ReadKey { key: key.to_vec() });
    worker
        .send(
            2,
            RedisRequest::Mutate {
                key: key.to_vec(),
                change: RedisMutation::SetString {
                    value: b"lost".to_vec(),
                    expected: b"new".to_vec(),
                },
            },
        )
        .unwrap();
    let failed = response(&worker);
    assert!(failed.connected);
    assert!(failed.result.unwrap_err().contains("changed"));
    let RedisData::Key(large) = run(&worker, RedisRequest::ReadKey { key: key.to_vec() }) else {
        panic!()
    };
    assert!(large.truncated);
    let RedisValue::String(bytes) = large.value else {
        panic!()
    };
    assert_eq!(bytes.len(), MAX_VALUE_BYTES);
    worker
        .send(
            3,
            RedisRequest::Mutate {
                key: key.to_vec(),
                change: RedisMutation::SetString {
                    value: vec![],
                    expected: bytes,
                },
            },
        )
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("截断"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn atomic_create_mutation_type_races_expiry_and_rename_never_overwrite() {
    let mock = Mock::start(vec![
        ping(),
        Box::new(|args| {
            assert_eq!(args[0], b"EVAL");
            let script = std::str::from_utf8(&args[1]).unwrap();
            assert!(script.find("'EXISTS'").unwrap() < script.find("'HSET'").unwrap());
            assert_eq!(
                &args[2..],
                &[
                    b"1".to_vec(),
                    b"new\0".to_vec(),
                    b"hash".to_vec(),
                    b"v\xff".to_vec(),
                    b"f\0".to_vec(),
                    b"0.0".to_vec()
                ]
            );
            integer(1)
        }),
        step(
            "EVAL",
            b"-ERR Key already exists; not overwritten\r\n".to_vec(),
        ),
        step("TYPE", b"+string\r\n".to_vec()), // Hash UI operation must stop before EVAL.
        step("TYPE", b"+hash\r\n".to_vec()),
        Box::new(|args| {
            assert_eq!(
                &args[4..],
                &[
                    b"hash".to_vec(),
                    b"HSET".to_vec(),
                    b"f".to_vec(),
                    b"v".to_vec()
                ]
            );
            assert!(
                std::str::from_utf8(&args[1])
                    .unwrap()
                    .contains("kind ~= ARGV[1]")
            );
            b"-ERR Key type changed or expired; reload first\r\n".to_vec()
        }),
        step("TYPE", b"+none\r\n".to_vec()),
        step("TYPE", b"+hash\r\n".to_vec()),
        Box::new(|args| {
            let script = std::str::from_utf8(&args[1]).unwrap();
            assert!(script.contains("'RENAMENX'"));
            assert!(!script.contains("'RENAME'"));
            assert_eq!(args[5], b"RENAMENX");
            b"-ERR Destination exists; not overwritten\r\n".to_vec()
        }),
        step("TYPE", b"+hash\r\n".to_vec()),
        step("EVAL", integer(0)),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let create = || RedisRequest::Create {
        key: b"new\0".to_vec(),
        kind: RedisKind::Hash,
        value: b"v\xff".to_vec(),
        field: b"f\0".to_vec(),
        score: 0.0,
    };
    assert!(matches!(
        run(&worker, create()),
        RedisData::Changed { deleted: false, .. }
    ));
    worker.send(2, create()).unwrap();
    assert!(
        response(&worker)
            .result
            .unwrap_err()
            .contains("already exists")
    );
    for expected in ["类型", "changed or expired", "不存在", "Destination exists"] {
        let change = if expected == "Destination exists" {
            RedisMutation::Rename {
                new_key: b"existing".to_vec(),
            }
        } else {
            RedisMutation::HashSet {
                field: b"f".to_vec(),
                value: b"v".to_vec(),
            }
        };
        worker
            .send(
                3,
                RedisRequest::Mutate {
                    key: b"new\0".to_vec(),
                    change,
                },
            )
            .unwrap();
        let failed = response(&worker);
        assert!(failed.connected);
        assert!(failed.result.unwrap_err().contains(expected));
    }
    assert!(matches!(
        run(
            &worker,
            RedisRequest::Mutate {
                key: b"new\0".to_vec(),
                change: RedisMutation::Delete
            }
        ),
        RedisData::Changed { deleted: true, .. }
    ));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn list_save_atomically_checks_original_element_before_setting() {
    fn check_script(args: &[Vec<u8>]) {
        assert_eq!(args[0], b"EVAL");
        let script = std::str::from_utf8(&args[1]).unwrap();
        let check = script
            .find("redis.call('LINDEX', KEYS[1], ARGV[3]) ~= ARGV[5]")
            .unwrap();
        let write = script
            .find("redis.call('LSET', KEYS[1], ARGV[3], ARGV[4])")
            .unwrap();
        assert!(check < write);
        assert_eq!(
            &args[2..],
            &[
                b"1".to_vec(),
                b"list\0".to_vec(),
                b"list".to_vec(),
                b"LSET".to_vec(),
                b"1".to_vec(),
                b"replacement\xff".to_vec(),
                b"original\0".to_vec(),
            ]
        );
    }
    let mock = Mock::start(vec![
        ping(),
        step("TYPE", b"+list\r\n".to_vec()),
        Box::new(|args| {
            check_script(&args);
            integer(1)
        }),
        step("TYPE", b"+list\r\n".to_vec()),
        Box::new(|args| {
            check_script(&args);
            b"-ERR List element changed or shifted; reload before saving\r\n".to_vec()
        }),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let change = || RedisRequest::Mutate {
        key: b"list\0".to_vec(),
        change: RedisMutation::ListSet {
            index: 1,
            value: b"replacement\xff".to_vec(),
            expected: b"original\0".to_vec(),
        },
    };
    assert!(matches!(
        run(&worker, change()),
        RedisData::Changed { deleted: false, .. }
    ));
    worker.send(2, change()).unwrap();
    let failed = response(&worker);
    assert!(failed.connected);
    assert!(failed.result.unwrap_err().contains("changed or shifted"));
    assert!(
        worker
            .send(
                3,
                RedisRequest::Mutate {
                    key: b"list\0".to_vec(),
                    change: RedisMutation::ListSet {
                        index: 1,
                        value: vec![],
                        expected: vec![0; MAX_VALUE_BYTES + 1]
                    },
                }
            )
            .is_err()
    );
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn uncertain_write_is_never_replayed_and_bounded_queue_never_blocks_ui() {
    let calls = Arc::new(Mutex::new(0));
    let recorded = calls.clone();
    let mock = Mock::start(vec![
        ping(),
        Box::new(move |args| {
            assert_eq!(args[0], b"SET");
            *recorded.lock().unwrap() += 1;
            Vec::new() // close after consuming write, before replying
        }),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    worker
        .send(
            2,
            RedisRequest::Command {
                args: parse_command("SET k v").unwrap(),
                confirmed: true,
            },
        )
        .unwrap();
    let failed = response(&worker);
    assert!(!failed.connected);
    let error = failed.result.unwrap_err();
    assert!(error.contains("写入结果未知"));
    assert!(error.contains("未自动重试"));
    worker
        .send(3, RedisRequest::ReadKey { key: b"k".to_vec() })
        .unwrap();
    assert!(response(&worker).result.is_err());
    assert_eq!(*calls.lock().unwrap(), 1);
    mock.finish();
    // Stop draining responses: both directions are bounded and send uses try_send.
    let worker = RedisWorker::start(|| {});
    let start = Instant::now();
    let mut full = false;
    for index in 0..1000 {
        if worker
            .send(index, RedisRequest::ReadKey { key: b"k".to_vec() })
            .is_err()
        {
            full = true;
            break;
        }
    }
    assert!(full);
    assert!(start.elapsed() < Duration::from_secs(1));
}
