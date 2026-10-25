use super::*;

#[test]
fn redis_profiles_and_secrets_are_separate_and_preserve_sql_entries() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    let mut redis = RedisProfile::default();
    assert_eq!(
        (redis.host.as_str(), redis.port, redis.database),
        ("localhost", 6379, 0)
    );
    assert!(redis.username.is_empty());
    assert!(redis.validate().is_ok());
    redis.ssh.enabled = true;
    redis.ssh.host = "ssh.test".into();
    let sql = Profile::default();
    let sql_secret = Credentials {
        database_password: "sql-password-keep".into(),
        ..Default::default()
    };
    let secret = Credentials {
        database_password: "redis-private-password-秘密".into(),
        ssh_secret: "redis-ssh-secret-秘密".into(),
    };
    assert!(store.load_redis().unwrap().is_empty());
    store.save(std::slice::from_ref(&sql)).unwrap();
    store.save_credentials(&sql, &sql_secret).unwrap();
    store.save_redis(std::slice::from_ref(&redis)).unwrap();
    store.save_redis_credentials(&redis, &secret).unwrap();
    let loaded = store.load_redis().unwrap();
    assert_eq!(loaded[0].id, redis.id);
    assert_eq!(
        store
            .redis_credentials(&loaded[0])
            .unwrap()
            .unwrap()
            .database_password,
        secret.database_password
    );
    let json = std::fs::read_to_string(directory.path().join("redis-connections.json")).unwrap();
    let document: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(document[0].get("password").is_none());
    assert!(document[0].get("database_password").is_none());
    assert!(!json.contains("ssh_secret"));
    for file in [
        "redis-connections.json",
        "connections.json",
        "credentials.enc",
    ] {
        let bytes = std::fs::read(directory.path().join(file)).unwrap();
        for secret in [
            &secret.database_password,
            &secret.ssh_secret,
            &sql_secret.database_password,
        ] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
            );
        }
    }
    redis.remember_password = false;
    store.save_redis_credentials(&redis, &secret).unwrap();
    let saved = store.redis_credentials(&redis).unwrap().unwrap();
    assert!(saved.database_password.is_empty());
    assert_eq!(saved.ssh_secret, secret.ssh_secret);
    redis.remember_password = true;
    assert!(
        store
            .redis_credentials(&redis)
            .unwrap()
            .unwrap()
            .database_password
            .is_empty()
    );
    redis.remember_password = false;
    redis.ssh.remember_secret = false;
    store.save_redis_credentials(&redis, &secret).unwrap();
    assert!(store.redis_credentials(&redis).unwrap().is_none());
    assert_eq!(
        store.credentials(&sql).unwrap().unwrap().database_password,
        sql_secret.database_password
    );
    assert_eq!(store.load().unwrap()[0].id, sql.id);
    let path = directory.path().join("redis-connections.json");
    std::fs::write(&path, b"broken").unwrap();
    assert!(store.load_redis().is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"broken");
}

#[test]
fn acl_binary_scan_database_and_command_output_use_one_connection() {
    let mock = Mock::start(vec![
        exact(&[b"AUTH", b"alice", b"secret@:/\0"], b"+OK\r\n".to_vec()),
        ping(),
        exact(
            &[b"SCAN", b"0", b"MATCH", b"*", b"COUNT", b"200"],
            scan(7, &[b"a\0\xff", b"normal"]),
        ),
        exact(
            &[b"SCAN", b"7", b"MATCH", b"a*", b"COUNT", b"200"],
            scan(0, &[b"a\0\xff"]),
        ),
        exact(&[b"SELECT", b"3"], b"+OK\r\n".to_vec()),
        exact(
            &[b"SELECT", b"999"],
            b"-ERR DB index is out of range\r\n".to_vec(),
        ),
        exact(&[b"ECHO", b"\0\xff"], bulk(b"\0\xff")),
    ]);
    let worker = RedisWorker::start(|| {});
    let mut profile = mock.profile.clone();
    profile.username = "alice".into();
    profile.database = 2;
    connect(
        &worker,
        profile,
        Credentials {
            database_password: "secret@:/\0".into(),
            ..Default::default()
        },
    );
    assert!(
        matches!(run(&worker, RedisRequest::Scan { cursor: 0, pattern: "*".into() }), RedisData::Keys { cursor: 7, keys } if keys[0] == b"a\0\xff")
    );
    assert!(matches!(
        run(
            &worker,
            RedisRequest::Scan {
                cursor: 7,
                pattern: "a*".into()
            }
        ),
        RedisData::Keys { cursor: 0, .. }
    ));
    run(&worker, RedisRequest::SelectDb(3));
    worker.send(9, RedisRequest::SelectDb(999)).unwrap();
    let failed = response(&worker);
    assert_eq!(failed.database, 3);
    assert!(failed.connected);
    assert!(failed.result.is_err());
    assert!(
        matches!(run(&worker, RedisRequest::Command { args: parse_command(r"ECHO \x00\xFF").unwrap(), confirmed: false }), RedisData::Output(text) if text == r"\x00\xFF")
    );
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn password_only_auth_and_auth_errors_do_not_echo_secrets() {
    let mock = Mock::start(vec![exact(
        &[b"AUTH", b"private-secret"],
        b"-WRONGPASS private-secret\r\n".to_vec(),
    )]);
    let worker = RedisWorker::start(|| {});
    worker
        .send(
            1,
            RedisRequest::Connect {
                profile: mock.profile.clone(),
                credentials: Credentials {
                    database_password: "private-secret".into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    let failed = response(&worker);
    assert!(!failed.connected);
    let error = failed.result.unwrap_err();
    assert!(!error.contains("private-secret"));
    assert!(error.contains("认证"));
    mock.finish();
}
