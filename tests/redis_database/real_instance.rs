use super::*;

#[test]
#[ignore = "requires an explicitly supplied disposable Redis: SQLSHOP_TEST_REDIS_PORT"]
fn disposable_real_redis_crud_ttl_and_binary_scan() {
    let port: u16 = std::env::var("SQLSHOP_TEST_REDIS_PORT")
        .expect("explicit disposable test port required")
        .parse()
        .unwrap();
    let worker = RedisWorker::start(|| {});
    connect(
        &worker,
        RedisProfile {
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        },
        Credentials::default(),
    );
    let prefix = format!("sqlshop-test:{}:", uuid::Uuid::new_v4());
    let key = [prefix.as_bytes(), b"\0\xff"].concat();
    for (kind, value, field) in [
        (RedisKind::String, b"a\0b".to_vec(), vec![]),
        (RedisKind::Hash, b"v\xff".to_vec(), b"f\0".to_vec()),
        (RedisKind::List, b"v\0".to_vec(), vec![]),
        (RedisKind::Set, b"v\xff".to_vec(), vec![]),
        (RedisKind::ZSet, b"v\0".to_vec(), vec![]),
    ] {
        run(
            &worker,
            RedisRequest::Create {
                key: key.clone(),
                kind: kind.clone(),
                value: value.clone(),
                field,
                score: 1.25,
            },
        );
        worker
            .send(
                2,
                RedisRequest::Create {
                    key: key.clone(),
                    kind: kind.clone(),
                    value: b"overwrite".to_vec(),
                    field: b"f".to_vec(),
                    score: 0.0,
                },
            )
            .unwrap();
        assert!(response(&worker).result.is_err());
        run(
            &worker,
            RedisRequest::Mutate {
                key: key.clone(),
                change: RedisMutation::Expire { seconds: 60 },
            },
        );
        let RedisData::Key(read) = run(&worker, RedisRequest::ReadKey { key: key.clone() }) else {
            panic!()
        };
        assert_eq!(read.kind, kind);
        assert!(read.ttl_ms > 0 && read.ttl_ms <= 60000);
        if kind == RedisKind::String {
            run(
                &worker,
                RedisRequest::Mutate {
                    key: key.clone(),
                    change: RedisMutation::SetString {
                        value: b"new\xff".to_vec(),
                        expected: value,
                    },
                },
            );
            let RedisData::Key(read) = run(&worker, RedisRequest::ReadKey { key: key.clone() })
            else {
                panic!()
            };
            assert!(read.ttl_ms > 0 && read.ttl_ms <= 60000);
        }
        let mut cursor = 0;
        let mut found = false;
        loop {
            let RedisData::Keys { cursor: next, keys } = run(
                &worker,
                RedisRequest::Scan {
                    cursor,
                    pattern: format!("{prefix}*"),
                },
            ) else {
                panic!()
            };
            found |= keys.contains(&key);
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        assert!(found);
        run(
            &worker,
            RedisRequest::Mutate {
                key: key.clone(),
                change: RedisMutation::Persist,
            },
        );
        run(
            &worker,
            RedisRequest::Mutate {
                key: key.clone(),
                change: RedisMutation::Delete,
            },
        );
    }
    run(&worker, RedisRequest::Disconnect);
}
