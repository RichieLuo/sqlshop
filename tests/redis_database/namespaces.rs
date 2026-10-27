use super::*;

#[test]
fn namespace_preview_escapes_literal_binary_prefix_and_deletes_only_deduplicated_snapshot() {
    let prefix = b"folder[?]*\\:\0:";
    let pattern = b"folder\\[\\?\\]\\*\\\\:\0:*";
    let first = [prefix.as_slice(), b"first"].concat();
    let second = [prefix.as_slice(), b"second"].concat();
    let parent = &prefix[..prefix.len() - 1];
    let expected = [first.clone(), second.clone(), prefix.to_vec()]
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let mock = Mock::start(vec![
        ping(),
        exact(&[b"SELECT", b"3"], b"+OK\r\n".to_vec()),
        exact(
            &[b"SCAN", b"0", b"MATCH", pattern, b"COUNT", b"200"],
            scan(7, &[&first, &first, parent, b"folderx:unexpected"]),
        ),
        exact(
            &[b"SCAN", b"7", b"MATCH", pattern, b"COUNT", b"200"],
            scan(0, &[&second, prefix, &second]),
        ),
        // Metadata reads do not invalidate a confirmation snapshot or change DB.
        exact(
            &[b"CONFIG", b"GET", b"databases"],
            array(vec![bulk(b"databases"), bulk(b"32")]),
        ),
        exact(&[b"INFO", b"keyspace"], bulk(b"# Keyspace\r\n")),
        Box::new(move |args| {
            assert_eq!(args[0], b"UNLINK");
            assert_eq!(args.len(), 4);
            assert_eq!(
                args[1..]
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>(),
                expected
            );
            integer(3)
        }),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    run(&worker, RedisRequest::SelectDb(3));
    let token = namespace_preview(&worker, prefix, 3);
    run(&worker, RedisRequest::Databases);
    worker
        .send(2, RedisRequest::DeleteNamespace { token })
        .unwrap();
    let result = response(&worker);
    assert_eq!(result.database, 3);
    assert!(result.connected);
    assert!(
        matches!(result.result.unwrap(), RedisData::NamespaceDeleted { prefix: p, deleted: 3 } if p == prefix)
    );
    worker
        .send(3, RedisRequest::DeleteNamespace { token })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("失效"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn large_namespace_preview_warns_and_deletes_all_10001_keys_in_batches() {
    let mut steps = vec![
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &[b"ns:old"]),
        ),
    ];
    for page in 0..11 {
        let keys: Vec<Vec<u8>> = (page * 1000..((page + 1) * 1000).min(10_001))
            .map(|index| format!("ns:{index}").into_bytes())
            .collect();
        let reply = scan(
            if page == 10 { 0 } else { page + 1 },
            &keys.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        );
        let cursor = page.to_string();
        steps.push(exact(
            &[
                b"SCAN",
                cursor.as_bytes(),
                b"MATCH",
                b"ns:*",
                b"COUNT",
                b"200",
            ],
            reply,
        ));
    }
    let deleted_keys = Arc::new(Mutex::new(std::collections::HashSet::new()));
    for batch in 0..101 {
        let deleted_keys = deleted_keys.clone();
        steps.push(Box::new(move |args| {
            let count = if batch == 100 { 1 } else { 100 };
            assert_eq!(args[0], b"UNLINK");
            assert_eq!(args.len(), count + 1);
            let mut deleted_keys = deleted_keys.lock().unwrap();
            for key in &args[1..] {
                assert!(deleted_keys.insert(key.clone()), "duplicate deletion");
            }
            integer(count as i64)
        }));
    }
    let mock = Mock::start(steps);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let old = namespace_preview(&worker, b"ns:", 1);
    let RedisData::NamespacePreview {
        token,
        count,
        notice,
        ..
    } = run(
        &worker,
        RedisRequest::PreviewNamespace {
            prefix: b"ns:".to_vec(),
        },
    )
    else {
        panic!("expected a complete namespace preview")
    };
    assert_eq!(count, 10_001);
    assert!(notice.is_some());
    assert_ne!(token, old);
    assert!(matches!(
        run(&worker, RedisRequest::DeleteNamespace { token }),
        RedisData::NamespaceDeleted {
            deleted: 10_001,
            ..
        }
    ));
    let expected = (0..10_001)
        .map(|index| format!("ns:{index}").into_bytes())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(*deleted_keys.lock().unwrap(), expected);
    worker
        .send(3, RedisRequest::DeleteNamespace { token: old })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("失效"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn namespace_key_names_over_4_mib_warn_and_remain_deletable() {
    let mut steps = vec![ping()];
    let mut expected = std::collections::HashSet::new();
    for page in 0..9 {
        let mut key = b"ns:".to_vec();
        key.resize(512 * 1024, b'a' + page as u8);
        let cursor = page.to_string();
        steps.push(exact(
            &[
                b"SCAN",
                cursor.as_bytes(),
                b"MATCH",
                b"ns:*",
                b"COUNT",
                b"200",
            ],
            scan(if page == 8 { 0 } else { page + 1 }, &[&key]),
        ));
        expected.insert(key);
    }
    steps.push(Box::new(move |args| {
        assert_eq!(args[0], b"UNLINK");
        assert_eq!(
            args[1..]
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
            expected
        );
        integer(9)
    }));
    let mock = Mock::start(steps);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let RedisData::NamespacePreview {
        token,
        count,
        notice,
        ..
    } = run(
        &worker,
        RedisRequest::PreviewNamespace {
            prefix: b"ns:".to_vec(),
        },
    )
    else {
        panic!("expected a complete namespace preview")
    };
    assert_eq!(count, 9);
    assert!(notice.is_some());
    assert!(matches!(
        run(&worker, RedisRequest::DeleteNamespace { token }),
        RedisData::NamespaceDeleted { deleted: 9, .. }
    ));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn namespace_preview_continues_past_10000_scan_pages() {
    let mut steps = vec![ping()];
    for page in 0..=10_000 {
        let cursor = page.to_string();
        steps.push(exact(
            &[
                b"SCAN",
                cursor.as_bytes(),
                b"MATCH",
                b"ns:*",
                b"COUNT",
                b"200",
            ],
            if page == 10_000 {
                scan(0, &[b"ns:last"])
            } else {
                scan(page + 1, &[])
            },
        ));
    }
    steps.push(exact(&[b"UNLINK", b"ns:last"], integer(1)));
    let mock = Mock::start(steps);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let RedisData::NamespacePreview {
        token,
        count,
        notice,
        ..
    } = run(
        &worker,
        RedisRequest::PreviewNamespace {
            prefix: b"ns:".to_vec(),
        },
    )
    else {
        panic!("expected a complete namespace preview")
    };
    assert_eq!(count, 1);
    assert!(notice.is_none());
    assert!(matches!(
        run(&worker, RedisRequest::DeleteNamespace { token }),
        RedisData::NamespaceDeleted { deleted: 1, .. }
    ));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn namespace_preview_can_take_longer_than_ten_seconds_overall() {
    let mut steps = vec![ping()];
    for page in 0..2 {
        steps.push(Box::new(move |args| {
            assert_eq!(args[0], b"SCAN");
            assert_eq!(args[1], page.to_string().as_bytes());
            std::thread::sleep(Duration::from_millis(5500));
            if page == 0 {
                scan(1, &[b"ns:first"])
            } else {
                scan(0, &[b"ns:last"])
            }
        }));
    }
    let mock = Mock::start(steps);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    namespace_preview(&worker, b"ns:", 2);
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn failed_namespace_scan_invalidates_the_previous_preview() {
    let mock = Mock::start(vec![
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &[b"ns:old"]),
        ),
        step("SCAN", b"-NOPERM SCAN denied\r\n".to_vec()),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let token = namespace_preview(&worker, b"ns:", 1);
    worker
        .send(
            2,
            RedisRequest::PreviewNamespace {
                prefix: b"ns:".to_vec(),
            },
        )
        .unwrap();
    let failed = response(&worker);
    assert!(failed.connected);
    assert!(failed.result.is_err());
    worker
        .send(3, RedisRequest::DeleteNamespace { token })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("失效"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn namespace_tokens_are_invalidated_by_db_selection_writes_and_mismatches() {
    let preview = || {
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &[b"ns:k"]),
        )
    };
    let mock = Mock::start(vec![
        ping(),
        preview(),
        exact(
            &[b"SELECT", b"999"],
            b"-ERR DB index is out of range\r\n".to_vec(),
        ),
        preview(),
        exact(&[b"SELECT", b"2"], b"+OK\r\n".to_vec()),
        preview(),
        exact(&[b"SET", b"other", b"v"], b"+OK\r\n".to_vec()),
        preview(),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    for select in [999, 2] {
        let token = namespace_preview(&worker, b"ns:", 1);
        worker.send(2, RedisRequest::SelectDb(select)).unwrap();
        let result = response(&worker);
        assert_eq!(result.database, if select == 999 { 0 } else { 2 });
        assert_eq!(result.result.is_err(), select == 999);
        worker
            .send(3, RedisRequest::DeleteNamespace { token })
            .unwrap();
        assert!(response(&worker).result.unwrap_err().contains("失效"));
    }
    let token = namespace_preview(&worker, b"ns:", 1);
    run(
        &worker,
        RedisRequest::Command {
            args: parse_command("SET other v").unwrap(),
            confirmed: true,
        },
    );
    worker
        .send(4, RedisRequest::DeleteNamespace { token })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("失效"));
    let token = namespace_preview(&worker, b"ns:", 1);
    worker
        .send(5, RedisRequest::DeleteNamespace { token: token + 1 })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("不匹配"));
    worker
        .send(6, RedisRequest::DeleteNamespace { token })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("失效"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn namespace_tokens_cannot_be_reused_after_disconnect_or_on_another_connection() {
    let first = Mock::start(vec![
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &[b"ns:k"]),
        ),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, first.profile.clone(), Credentials::default());
    let token = namespace_preview(&worker, b"ns:", 1);
    run(&worker, RedisRequest::Disconnect);
    first.finish();
    let second = Mock::start(vec![
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &[b"ns:k"]),
        ),
    ]);
    connect(&worker, second.profile.clone(), Credentials::default());
    let other = namespace_preview(&worker, b"ns:", 1);
    assert_ne!(token, other);
    worker
        .send(7, RedisRequest::DeleteNamespace { token })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("不匹配"));
    run(&worker, RedisRequest::Disconnect);
    second.finish();
}

#[test]
fn namespace_partial_failure_reports_confirmed_count_and_never_replays_unknown_batch() {
    let names: Vec<_> = (0..250)
        .map(|index| format!("ns:{index}").into_bytes())
        .collect();
    let seen = Arc::new(Mutex::new(std::collections::HashSet::<Vec<u8>>::new()));
    let mut steps = vec![
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &names.iter().map(Vec::as_slice).collect::<Vec<_>>()),
        ),
    ];
    for batch in 0..2 {
        let seen = seen.clone();
        steps.push(Box::new(move |args| {
            assert_eq!(args[0], b"UNLINK");
            assert_eq!(args.len(), 101);
            let mut seen = seen.lock().unwrap();
            for key in &args[1..] {
                assert!(seen.insert(key.clone()), "batch must never be replayed");
            }
            if batch == 0 { integer(100) } else { Vec::new() }
        }));
    }
    let mock = Mock::start(steps);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let token = namespace_preview(&worker, b"ns:", 250);
    worker
        .send(8, RedisRequest::DeleteNamespace { token })
        .unwrap();
    let failed = response(&worker);
    assert!(!failed.connected);
    let message = failed.result.unwrap_err();
    assert!(message.contains("已确认删除 100"), "{message}");
    assert!(message.contains("100 个 Key 的结果未知"), "{message}");
    assert!(message.contains("未自动重试"));
    assert_eq!(seen.lock().unwrap().len(), 200);
    worker
        .send(9, RedisRequest::DeleteNamespace { token })
        .unwrap();
    assert!(
        response(&worker)
            .result
            .unwrap_err()
            .contains("不会自动重连")
    );
    mock.finish();
}

#[test]
fn namespace_unlink_refusal_never_falls_back_to_del_or_reuses_token() {
    let mock = Mock::start(vec![
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &[b"ns:k"]),
        ),
        exact(
            &[b"UNLINK", b"ns:k"],
            b"-ERR unknown command UNLINK private-auth-secret\r\n".to_vec(),
        ),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let token = namespace_preview(&worker, b"ns:", 1);
    worker
        .send(3, RedisRequest::DeleteNamespace { token })
        .unwrap();
    let failed = response(&worker);
    assert!(failed.connected);
    let message = failed.result.unwrap_err();
    assert!(message.contains("已确认删除 0"));
    assert!(message.contains("Redis 4+ UNLINK"));
    assert!(!message.contains("private-auth-secret"));
    worker
        .send(4, RedisRequest::DeleteNamespace { token })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("失效"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn empty_namespace_preview_can_be_confirmed_without_sending_empty_unlink() {
    let mock = Mock::start(vec![
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"ns:*", b"COUNT", b"200"],
            scan(0, &[]),
        ),
    ]);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    for prefix in [b"".as_slice(), b"ns", b"*"] {
        assert!(
            worker
                .send(
                    0,
                    RedisRequest::PreviewNamespace {
                        prefix: prefix.to_vec()
                    }
                )
                .is_err()
        );
    }
    let token = namespace_preview(&worker, b"ns:", 0);
    assert!(matches!(
        run(&worker, RedisRequest::DeleteNamespace { token }),
        RedisData::NamespaceDeleted { deleted: 0, .. }
    ));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}
