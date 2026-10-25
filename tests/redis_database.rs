#[path = "redis_database/real_instance.rs"]
mod real_instance;

#[path = "redis_database/mutations.rs"]
mod mutations;

#[path = "redis_database/namespaces.rs"]
mod namespaces;

#[path = "redis_database/databases.rs"]
mod databases;

#[path = "redis_database/limits.rs"]
mod limits;

#[path = "redis_database/authentication.rs"]
mod authentication;

#[path = "redis_database/protocol.rs"]
mod protocol;

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
