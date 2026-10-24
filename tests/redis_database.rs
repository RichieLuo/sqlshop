use sqlshop::{
    model::{Credentials, Profile},
    redis_database::{
        MAX_COLLECTION_ITEMS, MAX_VALUE_BYTES, RedisData, RedisDatabase, RedisKind, RedisMutation,
        RedisProfile, RedisRequest, RedisResponse, RedisValue, RedisWorker,
        command_needs_confirmation, display_bytes, format_hex, parse_command, parse_hex,
    },
    storage::Store,
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[path = "support/ssh_gateway.rs"]
mod ssh_gateway;

type Step = Box<dyn FnOnce(Vec<Vec<u8>>) -> Vec<u8> + Send>;
struct Mock {
    profile: RedisProfile,
    task: JoinHandle<()>,
}
impl Mock {
    fn start(steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let profile = RedisProfile {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            ..Default::default()
        };
        let task = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(15)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for step in steps {
                let args = read_command(&mut reader);
                writer.write_all(&step(args)).unwrap();
                writer.flush().unwrap();
            }
        });
        Self { profile, task }
    }
    fn finish(self) {
        self.task.join().unwrap();
    }
}
fn read_command(reader: &mut BufReader<TcpStream>) -> Vec<Vec<u8>> {
    fn line(reader: &mut BufReader<TcpStream>) -> Vec<u8> {
        let mut bytes = Vec::new();
        assert!(
            reader.read_until(b'\n', &mut bytes).unwrap() > 0,
            "expected command, got EOF"
        );
        assert!(bytes.ends_with(b"\r\n"));
        bytes.truncate(bytes.len() - 2);
        bytes
    }
    let count = line(reader);
    assert_eq!(count[0], b'*');
    let count: usize = std::str::from_utf8(&count[1..]).unwrap().parse().unwrap();
    (0..count)
        .map(|_| {
            let size = line(reader);
            assert_eq!(size[0], b'$');
            let size: usize = std::str::from_utf8(&size[1..]).unwrap().parse().unwrap();
            let mut bytes = vec![0; size];
            reader.read_exact(&mut bytes).unwrap();
            assert_eq!(line(reader), b"");
            bytes
        })
        .collect()
}
fn step(name: &'static str, response: Vec<u8>) -> Step {
    Box::new(move |args| {
        assert_eq!(args[0], name.as_bytes());
        response
    })
}
fn exact(args: &[&[u8]], response: Vec<u8>) -> Step {
    let expected: Vec<Vec<u8>> = args.iter().map(|arg| arg.to_vec()).collect();
    Box::new(move |args| {
        assert_eq!(args, expected);
        response
    })
}
fn bulk(bytes: &[u8]) -> Vec<u8> {
    let mut result = format!("${}\r\n", bytes.len()).into_bytes();
    result.extend(bytes);
    result.extend(b"\r\n");
    result
}
fn array(values: Vec<Vec<u8>>) -> Vec<u8> {
    let mut result = format!("*{}\r\n", values.len()).into_bytes();
    for value in values {
        result.extend(value);
    }
    result
}
fn integer(number: i64) -> Vec<u8> {
    format!(":{number}\r\n").into_bytes()
}
fn scan(cursor: u64, keys: &[&[u8]]) -> Vec<u8> {
    array(vec![
        bulk(cursor.to_string().as_bytes()),
        array(keys.iter().map(|key| bulk(key)).collect()),
    ])
}
fn ping() -> Step {
    exact(&[b"PING"], b"+PONG\r\n".to_vec())
}
fn connect(worker: &RedisWorker, profile: RedisProfile, credentials: Credentials) {
    worker
        .send(
            1,
            RedisRequest::Connect {
                profile,
                credentials,
            },
        )
        .unwrap();
    let result = response(worker);
    assert!(result.connected);
    assert_eq!(result.database, 0);
    assert!(matches!(
        result.result.unwrap(),
        RedisData::Connected { database: 0, .. }
    ));
}
fn response(worker: &RedisWorker) -> RedisResponse {
    worker
        .receiver
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
}
fn run(worker: &RedisWorker, request: RedisRequest) -> RedisData {
    worker.send(42, request).unwrap();
    let result = response(worker);
    assert_eq!(result.generation, 42);
    result.result.unwrap()
}

#[test]
fn parser_is_binary_safe_and_rejects_incomplete_input() {
    assert_eq!(
        parse_command(r#"SET 'a b' "中\x00\xFF\n\t\"\\""#).unwrap(),
        vec![
            b"SET".to_vec(),
            b"a b".to_vec(),
            ["中".as_bytes(), b"\0\xff\n\t\"\\"].concat()
        ]
    );
    assert_eq!(
        parse_command("SET a ''").unwrap(),
        vec![b"SET".to_vec(), b"a".to_vec(), vec![]]
    );
    assert_eq!(
        parse_command(r#"SET ab"cd" e\ f"#).unwrap()[1..],
        [b"abcd".to_vec(), b"e f".to_vec()]
    );
    for input in [
        "",
        "  ",
        "''",
        "GET 'open",
        "GET \"open",
        "GET a\\",
        r"GET \x0",
        r"GET \xGG",
        r"GET \q",
    ] {
        assert!(parse_command(input).is_err(), "{input}");
    }
    assert!(!command_needs_confirmation(
        &parse_command("gEt a").unwrap()
    ));
    assert!(command_needs_confirmation(
        &parse_command("SET a b").unwrap()
    ));
    assert!(command_needs_confirmation(
        &parse_command("EVAL script 0").unwrap()
    ));
    assert!(command_needs_confirmation(&[]));
    assert_eq!(display_bytes("中文".as_bytes()), "中文");
    assert_eq!(display_bytes(b"a\0\xff"), r"\x61\x00\xFF");
    for bytes in [vec![], vec![0, 255, 127], "中文\n".as_bytes().to_vec()] {
        assert_eq!(parse_hex(&format_hex(&bytes)).unwrap(), bytes);
    }
    assert_eq!(parse_hex("00 FF\n7f").unwrap(), [0, 255, 127]);
    for input in ["0", "0x12", "GG", "FF f"] {
        assert!(parse_hex(input).is_err());
    }
}

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

fn namespace_preview(worker: &RedisWorker, prefix: &[u8], expected: usize) -> u64 {
    let RedisData::NamespacePreview {
        token,
        prefix: returned,
        count,
    } = run(
        worker,
        RedisRequest::PreviewNamespace {
            prefix: prefix.to_vec(),
        },
    )
    else {
        panic!()
    };
    assert_eq!(returned, prefix);
    assert_eq!(count, expected);
    token
}

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
fn namespace_preview_rejects_10001_unique_keys_and_never_offers_partial_snapshot() {
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
    let mock = Mock::start(steps);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
    let old = namespace_preview(&worker, b"ns:", 1);
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
    assert!(failed.result.unwrap_err().contains("10000"));
    worker
        .send(3, RedisRequest::DeleteNamespace { token: old })
        .unwrap();
    assert!(response(&worker).result.unwrap_err().contains("失效"));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

#[test]
fn namespace_key_name_budget_is_enforced_across_scan_pages() {
    let mut steps = vec![ping()];
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
    }
    let mock = Mock::start(steps);
    let worker = RedisWorker::start(|| {});
    connect(&worker, mock.profile.clone(), Credentials::default());
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
    assert!(failed.result.unwrap_err().contains("4 MiB"));
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
fn json_formatting_preserves_arbitrary_precision_numbers() {
    let input = r#"{"id":123456789012345678901234567890,"value":0.12345678901234567890123456789}"#;
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    let formatted = serde_json::to_string_pretty(&value).unwrap();
    assert!(formatted.contains("123456789012345678901234567890"));
    assert!(formatted.contains("0.12345678901234567890123456789"));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_connects_to_mock_redis_with_verified_host_key() {
    let mock = Mock::start(vec![ping(), exact(&[b"ECHO", b"ssh\0"], bulk(b"ssh\0"))]);
    let gateway = ssh_gateway::Gateway::start("127.0.0.1", mock.profile.port)
        .await
        .unwrap();
    let mut profile = mock.profile.clone();
    profile.ssh = gateway.config.clone();
    let worker = RedisWorker::start(|| {});
    connect(
        &worker,
        profile,
        Credentials {
            ssh_secret: ssh_gateway::PASSWORD.into(),
            ..Default::default()
        },
    );
    assert!(matches!(
        run(
            &worker,
            RedisRequest::Command {
                args: vec![b"ECHO".to_vec(), b"ssh\0".to_vec()],
                confirmed: false
            }
        ),
        RedisData::Output(_)
    ));
    run(&worker, RedisRequest::Disconnect);
    mock.finish();
}

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
