use super::*;

type HashEntries = Vec<(Vec<u8>, Vec<u8>)>;

pub(super) async fn handle_request(
    connection: &mut Option<RedisConnection>,
    database: &mut u32,
    request: RedisRequest,
) -> Result<RedisData> {
    validate_request(&request)?;
    match request {
        RedisRequest::Connect {
            profile,
            credentials,
        } => {
            // Explicit connection attempts replace the previous session, even on failure.
            *connection = None;
            let mut opened = RedisConnection::open(&profile, &credentials).await?;
            let _: String = opened
                .query(&redis::cmd("PING"), false)
                .await
                .map_err(|_| anyhow::anyhow!("Redis 连通性检查失败，请核实连接及 AUTH/ACL 配置"))?;
            *database = 0;
            *connection = Some(opened);
            Ok(RedisData::Connected {
                database: *database,
                server: format!("{}:{}", profile.host, profile.port),
            })
        }
        RedisRequest::Disconnect => {
            *connection = None;
            Ok(RedisData::Disconnected)
        }
        request => {
            let current = connection
                .as_mut()
                .context("请先连接 Redis；不会自动重连")?;
            current.write_started = false;
            let namespace_delete = matches!(&request, RedisRequest::DeleteNamespace { .. });
            let result = execute(current, database, request).await;
            let broken = current.broken;
            let write = current.write_started;
            if broken {
                *connection = None;
                if write {
                    // Namespace deletion already reports confirmed progress and the
                    // unknown batch; do not discard that with the generic write error.
                    if namespace_delete {
                        return result;
                    }
                    return Err(anyhow::anyhow!(
                        "写入结果未知：连接中断或响应超限；操作可能已执行，未自动重试。请手动重连并核实数据。{}",
                        result
                            .err()
                            .map(|error| format!(" {error:#}"))
                            .unwrap_or_default()
                    ));
                }
            }
            result
        }
    }
}

async fn execute(
    connection: &mut RedisConnection,
    database: &mut u32,
    request: RedisRequest,
) -> Result<RedisData> {
    match request {
        RedisRequest::Databases => databases(connection, *database).await,
        RedisRequest::PreviewNamespace { prefix } => {
            preview_namespace(connection, *database, prefix).await
        }
        RedisRequest::DeleteNamespace { token } => {
            delete_namespace(connection, *database, token).await
        }
        RedisRequest::SelectDb(number) => {
            // Even a failed selection cancels a previously confirmed namespace snapshot.
            connection.namespace = None;
            let _: () = connection
                .query(redis::cmd("SELECT").arg(number), false)
                .await?;
            *database = number;
            connection.strings.clear();
            Ok(RedisData::Database(number))
        }
        RedisRequest::Scan { cursor, pattern } => {
            let (cursor, keys): (u64, Vec<Vec<u8>>) = connection
                .query(
                    redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(if pattern.is_empty() { "*" } else { &pattern })
                        .arg("COUNT")
                        .arg(MAX_COLLECTION_ITEMS),
                    false,
                )
                .await?;
            // Never drop keys from a SCAN page: that would silently lose them on
            // the next cursor. Oversized pages fail explicitly at the transport cap.
            Ok(RedisData::Keys { cursor, keys })
        }
        RedisRequest::ReadKey { key } => Ok(RedisData::Key(read_key(connection, key).await?)),
        RedisRequest::Mutate { key, change } => {
            connection.namespace = None;
            mutate(connection, key, change).await
        }
        RedisRequest::Create {
            key,
            kind,
            value,
            field,
            score,
        } => {
            // A single script checks nonexistence and creates the initial element;
            // no temporary key, no overwrite window, and no retry on a lost reply.
            let script = r#"
if redis.call('EXISTS', KEYS[1]) ~= 0 then return redis.error_reply('Key already exists; not overwritten') end
local kind = ARGV[1]
if kind == 'string' then redis.call('SET', KEYS[1], ARGV[2])
elseif kind == 'hash' then redis.call('HSET', KEYS[1], ARGV[3], ARGV[2])
elseif kind == 'list' then redis.call('RPUSH', KEYS[1], ARGV[2])
elseif kind == 'set' then redis.call('SADD', KEYS[1], ARGV[2])
elseif kind == 'zset' then redis.call('ZADD', KEYS[1], ARGV[4], ARGV[2])
else return redis.error_reply('Unsupported type') end
return 1
"#;
            let _: i64 = connection
                .query(
                    redis::cmd("EVAL")
                        .arg(script)
                        .arg(1)
                        .arg(&key)
                        .arg(kind.label())
                        .arg(value)
                        .arg(field)
                        .arg(score),
                    true,
                )
                .await?;
            connection.strings.remove(&key);
            Ok(RedisData::Changed {
                key,
                deleted: false,
            })
        }
        RedisRequest::Command { args, confirmed } => {
            let name = validate_command(&args, confirmed)?;
            let write = !readonly_command(&args);
            let mut command = redis::cmd(&name);
            for arg in &args[1..] {
                command.arg(arg);
            }
            let value: Value = connection.query(&command, write).await?;
            if write {
                connection.strings.clear();
            }
            Ok(RedisData::Output(format_output(&value)))
        }
        _ => unreachable!("connection requests handled above"),
    }
}

const MAX_NAMESPACE_KEYS: usize = 10_000;
const MAX_NAMESPACE_KEY_BYTES: usize = 4 * 1024 * 1024;
const MAX_NAMESPACE_SCAN_PAGES: usize = 10_000;
const NAMESPACE_DELETE_BATCH: usize = 100;
static NEXT_NAMESPACE_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn namespace_pattern(prefix: &[u8]) -> Vec<u8> {
    let mut pattern = Vec::with_capacity(prefix.len().saturating_mul(2).saturating_add(1));
    for &byte in prefix {
        if matches!(byte, b'[' | b']' | b'?' | b'*' | b'\\') {
            pattern.push(b'\\');
        }
        pattern.push(byte);
    }
    pattern.push(b'*');
    pattern
}

async fn preview_namespace(
    connection: &mut RedisConnection,
    database: u32,
    prefix: Vec<u8>,
) -> Result<RedisData> {
    connection.namespace = None;
    let pattern = namespace_pattern(&prefix);
    let scan = async {
        let mut cursor = 0;
        let mut seen = HashSet::new();
        let mut key_bytes = 0usize;
        for _ in 0..MAX_NAMESPACE_SCAN_PAGES {
            let (next, page): (u64, Vec<Vec<u8>>) = connection
                .query(
                    redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(&pattern)
                        .arg("COUNT")
                        .arg(MAX_COLLECTION_ITEMS),
                    false,
                )
                .await?;
            for key in page {
                // MATCH is not a security boundary: check literal binary identity as well.
                // A parent key "foo" is not in namespace "foo:".
                if !key.starts_with(&prefix) || seen.contains(&key) {
                    continue;
                }
                anyhow::ensure!(
                    seen.len() < MAX_NAMESPACE_KEYS
                        && key.len() <= MAX_NAMESPACE_KEY_BYTES - key_bytes,
                    "命名空间预览超过安全上限（10000 个 Key / 4 MiB 键名）；未生成删除令牌，请缩小前缀"
                );
                key_bytes += key.len();
                seen.insert(key);
            }
            cursor = next;
            if cursor == 0 {
                return Ok::<_, anyhow::Error>(seen.into_iter().collect::<Vec<_>>());
            }
        }
        bail!("命名空间扫描页数超过安全上限；预览不完整，未生成删除令牌")
    };
    let keys = match tokio::time::timeout(TIMEOUT, scan).await {
        Ok(result) => result?,
        Err(_) => {
            connection.broken = true;
            bail!("命名空间完整预览超时，连接已关闭；未生成删除令牌")
        }
    };
    let token = NEXT_NAMESPACE_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |token| {
            token.checked_add(1)
        })
        .map_err(|_| anyhow::anyhow!("命名空间删除令牌耗尽，请重新启动应用"))?;
    let count = keys.len();
    connection.namespace = Some(super::transport::NamespaceSnapshot {
        token,
        database,
        prefix: prefix.clone(),
        keys,
    });
    // SCAN is not an atomic server snapshot. Only the deduplicated names observed
    // by this completed traversal are eligible; names added later are not scanned again.
    Ok(RedisData::NamespacePreview {
        token,
        prefix,
        count,
    })
}

async fn delete_namespace(
    connection: &mut RedisConnection,
    database: u32,
    token: u64,
) -> Result<RedisData> {
    // Single-use, even on rejection/partial failure. No retry can replay a batch.
    let snapshot = connection
        .namespace
        .take()
        .context("命名空间预览已失效；请重新预览并确认")?;
    anyhow::ensure!(
        snapshot.token == token && snapshot.database == database,
        "命名空间预览令牌与当前连接/DB 不匹配；请重新预览并确认"
    );
    connection.strings.clear();
    let mut deleted = 0u64;
    for keys in snapshot.keys.chunks(NAMESPACE_DELETE_BATCH) {
        let value = match connection
            .query::<Value>(redis::cmd("UNLINK").arg(keys), true)
            .await
        {
            Ok(value) => value,
            Err(_) if connection.broken => bail!(
                "命名空间删除中断：已确认删除 {deleted} 个 Key；当前批次 {} 个 Key 的结果未知，其余批次未发送；未自动重试，请重新连接并核实",
                keys.len()
            ),
            Err(_) => bail!(
                "命名空间删除失败：已确认删除 {deleted} 个 Key；当前批次被拒绝，其余批次未发送；要求 Redis 4+ UNLINK 支持及权限，不会回退 DEL 或自动重试"
            ),
        };
        match value {
            Value::Int(count) if count >= 0 && count as usize <= keys.len() => {
                deleted += count as u64
            }
            _ => {
                connection.broken = true;
                bail!(
                    "命名空间删除响应无效：已确认删除 {deleted} 个 Key；当前批次 {} 个 Key 的结果未知，其余批次未发送；未自动重试，连接已关闭",
                    keys.len()
                )
            }
        }
    }
    Ok(RedisData::NamespaceDeleted {
        prefix: snapshot.prefix,
        deleted,
    })
}

// Enumeration is metadata-only: never probe a DB with SELECT, DBSIZE or SCAN.
const MAX_DATABASES: usize = 4096;

async fn databases(connection: &mut RedisConnection, current: u32) -> Result<RedisData> {
    let mut notices = Vec::new();
    let configured = match connection
        .query::<Value>(redis::cmd("CONFIG").arg("GET").arg("databases"), false)
        .await
    {
        Ok(value) => match configured_database_count(value) {
            Some(count) => Some(count),
            None => {
                notices.push("CONFIG GET databases 返回无效的数据库数量；只能显示 INFO keyspace 活跃 DB 和当前 DB，无法枚举空 DB".to_owned());
                None
            }
        },
        Err(_) if connection.broken => {
            bail!("Redis 数据库枚举失败：连接中断或响应超过安全上限，请手动重连")
        }
        Err(_) => {
            // Do not forward server diagnostics: they may contain auth/config secrets.
            notices.push("CONFIG GET databases 不可用（权限不足或命令禁用）；只能显示 INFO keyspace 活跃 DB 和当前 DB，无法枚举空 DB".to_owned());
            None
        }
    };
    let keyspace = match connection
        .query::<Vec<u8>>(redis::cmd("INFO").arg("keyspace"), false)
        .await
    {
        Ok(bytes) => {
            let info = parse_keyspace(&bytes);
            if !info.zero_for_absent {
                notices.push("INFO keyspace 部分内容无效或已截断；缺失的 Key 数量未知".to_owned());
            }
            Some(info)
        }
        Err(_) if connection.broken => {
            bail!("Redis 数据库枚举失败：连接中断或响应超过安全上限，请手动重连")
        }
        Err(_) => {
            notices.push("INFO keyspace 不可用；Key 数量未知".to_owned());
            None
        }
    };
    let keys_for = |index| {
        keyspace.as_ref().and_then(|info| {
            info.active
                .get(&index)
                .copied()
                .unwrap_or_else(|| info.zero_for_absent.then_some(0))
        })
    };
    let mut complete = configured.is_some();
    let mut result = if let Some(count) = configured {
        if count > MAX_DATABASES as u64 {
            complete = false;
            notices.push(format!("数据库列表已截断（truncated）：服务器配置 {count} 个 DB，最多显示 {MAX_DATABASES} 个"));
        }
        if u64::from(current) >= count {
            complete = false;
            notices.push("当前 DB 超出 CONFIG 返回的范围；保留当前 DB，列表可能不完整".to_owned());
        }
        (0..count.min(MAX_DATABASES as u64) as u32)
            .map(|index| RedisDatabase {
                index,
                keys: keys_for(index),
            })
            .collect::<Vec<_>>()
    } else {
        if keyspace.as_ref().is_some_and(|info| info.truncated) {
            notices.push(format!(
                "活跃数据库列表已截断（truncated）：最多显示 {MAX_DATABASES} 个 DB"
            ));
        }
        keyspace.as_ref().map_or_else(Vec::new, |info| {
            info.active
                .iter()
                .map(|(&index, &keys)| RedisDatabase { index, keys })
                .collect()
        })
    };
    if !result.iter().any(|db| db.index == current) {
        if result.len() == MAX_DATABASES {
            result.pop();
            complete = false;
            notices.push("列表已截断（truncated），保留当前 DB".to_owned());
        }
        result.push(RedisDatabase {
            index: current,
            keys: keys_for(current),
        });
        result.sort_by_key(|db| db.index);
    }
    Ok(RedisData::Databases {
        databases: result,
        complete,
        notice: (!notices.is_empty()).then(|| notices.join("；")),
    })
}

fn unsigned_decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > 20 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn configured_database_count(value: Value) -> Option<u64> {
    let Value::Array(values) = value else {
        return None;
    };
    let [Value::BulkString(name), Value::BulkString(count)] = values.as_slice() else {
        return None;
    };
    if !name.eq_ignore_ascii_case(b"databases") {
        return None;
    }
    unsigned_decimal(count).filter(|count| *count > 0)
}

struct KeyspaceInfo {
    active: std::collections::BTreeMap<u32, Option<u64>>,
    zero_for_absent: bool,
    truncated: bool,
}
fn parse_keyspace(bytes: &[u8]) -> KeyspaceInfo {
    let mut info = KeyspaceInfo {
        active: Default::default(),
        zero_for_absent: true,
        truncated: false,
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        info.zero_for_absent = false;
        return info;
    };
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let Some((name, fields)) = line.split_once(':') else {
            info.zero_for_absent = false;
            continue;
        };
        let Some(index) = name
            .strip_prefix("db")
            .and_then(|index| unsigned_decimal(index.as_bytes()))
            .and_then(|index| u32::try_from(index).ok())
        else {
            info.zero_for_absent = false;
            continue;
        };
        if info.active.len() >= MAX_DATABASES && !info.active.contains_key(&index) {
            info.truncated = true;
            info.zero_for_absent = false;
            continue;
        }
        let mut counts = fields
            .split(',')
            .filter_map(|field| field.strip_prefix("keys="));
        let keys = counts
            .next()
            .and_then(|count| unsigned_decimal(count.as_bytes()));
        let keys = if counts.next().is_some() { None } else { keys };
        if keys.is_none() {
            info.zero_for_absent = false;
        }
        if info.active.insert(index, keys).is_some() {
            info.active.insert(index, None);
            info.zero_for_absent = false;
        }
    }
    info
}

async fn read_key(connection: &mut RedisConnection, key: Vec<u8>) -> Result<RedisKey> {
    connection.strings.clear();
    let kind = connection.kind(&key).await?;
    let ttl_ms: i64 = connection
        .query(redis::cmd("PTTL").arg(&key), false)
        .await?;
    anyhow::ensure!(ttl_ms != -2, "Key 不存在或已过期");
    let size_command = match kind {
        RedisKind::String => Some("STRLEN"),
        RedisKind::Hash => Some("HLEN"),
        RedisKind::List => Some("LLEN"),
        RedisKind::Set => Some("SCARD"),
        RedisKind::ZSet => Some("ZCARD"),
        RedisKind::Stream => Some("XLEN"),
        _ => None,
    };
    let size = if let Some(command) = size_command {
        connection
            .query::<u64>(redis::cmd(command).arg(&key), false)
            .await?
    } else {
        0
    };
    let mut truncated = false;
    let mut budget = MAX_VALUE_BYTES;
    let value = match &kind {
        RedisKind::String => {
            // GETRANGE, not GET followed by truncation, bounds data at the server.
            let bytes: Vec<u8> = connection
                .query(
                    redis::cmd("GETRANGE")
                        .arg(&key)
                        .arg(0)
                        .arg(MAX_VALUE_BYTES - 1),
                    false,
                )
                .await?;
            truncated = size > bytes.len() as u64;
            if !truncated {
                connection.strings.insert(key.clone(), bytes.clone());
            }
            RedisValue::String(bytes)
        }
        RedisKind::Hash => {
            let (cursor, pairs): (u64, HashEntries) = connection
                .query(
                    redis::cmd("HSCAN")
                        .arg(&key)
                        .arg(0)
                        .arg("COUNT")
                        .arg(MAX_COLLECTION_ITEMS),
                    false,
                )
                .await?;
            let mut seen = HashSet::new();
            let mut result = Vec::new();
            for (field, value) in pairs {
                if !seen.insert(field.clone()) {
                    continue;
                }
                // Returned fields stay editable: never expose a partial value.
                if result.len() >= MAX_COLLECTION_ITEMS
                    || field.len() > budget
                    || value.len() > budget - field.len()
                {
                    truncated = true;
                    break;
                }
                budget -= field.len() + value.len();
                result.push((field, value));
            }
            truncated |= cursor != 0 || size > result.len() as u64;
            RedisValue::Hash(result)
        }
        RedisKind::List => {
            let items: Vec<Vec<u8>> = connection
                .query(
                    redis::cmd("LRANGE")
                        .arg(&key)
                        .arg(0)
                        .arg(MAX_COLLECTION_ITEMS - 1),
                    false,
                )
                .await?;
            let mut result = Vec::new();
            for item in items {
                // Keep a complete prefix so list indices still identify the
                // original elements and saving cannot overwrite with a preview.
                if result.len() >= MAX_COLLECTION_ITEMS || item.len() > budget {
                    truncated = true;
                    break;
                }
                budget -= item.len();
                result.push(item);
            }
            truncated |= size > result.len() as u64;
            RedisValue::List(result)
        }
        RedisKind::Set => {
            let (cursor, items): (u64, Vec<Vec<u8>>) = connection
                .query(
                    redis::cmd("SSCAN")
                        .arg(&key)
                        .arg(0)
                        .arg("COUNT")
                        .arg(MAX_COLLECTION_ITEMS),
                    false,
                )
                .await?;
            let mut seen = HashSet::new();
            let mut result = Vec::new();
            for item in items {
                if !seen.insert(item.clone()) {
                    continue;
                }
                // Set members are identities: never expose a clipped identity as editable.
                if result.len() >= MAX_COLLECTION_ITEMS || item.len() > budget {
                    truncated = true;
                    break;
                }
                budget -= item.len();
                result.push(item);
            }
            truncated |= cursor != 0 || size > result.len() as u64;
            RedisValue::Set(result)
        }
        RedisKind::ZSet => {
            let items: Vec<(Vec<u8>, f64)> = connection
                .query(
                    redis::cmd("ZRANGE")
                        .arg(&key)
                        .arg(0)
                        .arg(MAX_COLLECTION_ITEMS - 1)
                        .arg("WITHSCORES"),
                    false,
                )
                .await?;
            let mut result = Vec::new();
            for (member, score) in items {
                if result.len() >= MAX_COLLECTION_ITEMS || member.len() > budget {
                    truncated = true;
                    break;
                }
                budget -= member.len();
                result.push((member, score));
            }
            truncated |= size > result.len() as u64;
            RedisValue::ZSet(result)
        }
        _ => RedisValue::Unsupported(format!("{} 暂只读；不读取无界数据", kind.label())),
    };
    // TYPE/size/value are not a snapshot; a changed/deleted key is never saved
    // blindly. Mutation scripts re-check type and strings compare the exact read.
    Ok(RedisKey {
        key,
        kind,
        ttl_ms,
        size,
        truncated,
        value,
    })
}

async fn mutate(
    connection: &mut RedisConnection,
    key: Vec<u8>,
    change: RedisMutation,
) -> Result<RedisData> {
    let kind = connection.kind(&key).await?;
    anyhow::ensure!(
        matches!(
            kind,
            RedisKind::String
                | RedisKind::Hash
                | RedisKind::List
                | RedisKind::Set
                | RedisKind::ZSet
        ),
        "Stream/未知 Redis 类型暂只读"
    );
    let mut args: Vec<Vec<u8>> = Vec::new();
    let operation;
    let mut changed_key = key.clone();
    match change {
        RedisMutation::SetString { value, expected } => {
            anyhow::ensure!(kind == RedisKind::String, "Key 类型不是 string");
            anyhow::ensure!(
                connection.strings.get(&key) == Some(&expected),
                "请先完整读取此 String；截断值或未经读取的原值不能保存"
            );
            operation = "STRING";
            args.push(value);
            args.push(expected);
        }
        RedisMutation::HashSet { field, value } => {
            anyhow::ensure!(kind == RedisKind::Hash, "Key 类型不是 hash");
            operation = "HSET";
            args.extend([field, value]);
        }
        RedisMutation::HashDelete { field } => {
            anyhow::ensure!(kind == RedisKind::Hash, "Key 类型不是 hash");
            operation = "HDEL";
            args.push(field);
        }
        RedisMutation::ListSet {
            index,
            value,
            expected,
        } => {
            anyhow::ensure!(kind == RedisKind::List, "Key 类型不是 list");
            operation = "LSET";
            args.extend([index.to_string().into_bytes(), value, expected]);
        }
        RedisMutation::ListPush { value } => {
            anyhow::ensure!(kind == RedisKind::List, "Key 类型不是 list");
            operation = "RPUSH";
            args.push(value);
        }
        RedisMutation::SetAdd { value } => {
            anyhow::ensure!(kind == RedisKind::Set, "Key 类型不是 set");
            operation = "SADD";
            args.push(value);
        }
        RedisMutation::SetRemove { value } => {
            anyhow::ensure!(kind == RedisKind::Set, "Key 类型不是 set");
            operation = "SREM";
            args.push(value);
        }
        RedisMutation::ZSetAdd { score, value } => {
            anyhow::ensure!(kind == RedisKind::ZSet, "Key 类型不是 zset");
            check_score(score)?;
            operation = "ZADD";
            args.extend([score.to_string().into_bytes(), value]);
        }
        RedisMutation::ZSetRemove { value } => {
            anyhow::ensure!(kind == RedisKind::ZSet, "Key 类型不是 zset");
            operation = "ZREM";
            args.push(value);
        }
        RedisMutation::Expire { seconds } => {
            operation = "EXPIRE";
            args.push(seconds.to_string().into_bytes());
        }
        RedisMutation::Persist => {
            operation = "PERSIST";
        }
        RedisMutation::Rename { new_key } => {
            operation = "RENAMENX";
            changed_key = new_key.clone();
            args.push(new_key);
        }
        RedisMutation::Delete => {
            operation = "DEL";
        }
    }
    let script = r#"
local kind = redis.call('TYPE', KEYS[1]).ok
if kind ~= ARGV[1] then return redis.error_reply('Key type changed or expired; reload first') end
local op = ARGV[2]
if op == 'STRING' then
    if redis.call('STRLEN', KEYS[1]) > 1048576 then return redis.error_reply('Truncated String cannot be saved') end
    if redis.call('GET', KEYS[1]) ~= ARGV[4] then return redis.error_reply('String changed; reload before saving') end
    redis.call('SET', KEYS[1], ARGV[3], 'KEEPTTL')
elseif op == 'LSET' then
    if redis.call('LINDEX', KEYS[1], ARGV[3]) ~= ARGV[5] then return redis.error_reply('List element changed or shifted; reload before saving') end
    redis.call('LSET', KEYS[1], ARGV[3], ARGV[4])
elseif op == 'RENAMENX' then
    if redis.call('RENAMENX', KEYS[1], ARGV[3]) == 0 then return redis.error_reply('Destination exists; not overwritten') end
else
    redis.call(op, KEYS[1], unpack(ARGV, 3))
end
return redis.call('EXISTS', KEYS[1])
"#;
    let mut command = redis::cmd("EVAL");
    command
        .arg(script)
        .arg(1)
        .arg(&key)
        .arg(kind.label())
        .arg(operation);
    for arg in args {
        command.arg(arg);
    }
    let exists: i64 = connection.query(&command, true).await?;
    connection.strings.clear();
    Ok(RedisData::Changed {
        key: changed_key,
        deleted: exists == 0 && operation != "RENAMENX",
    })
}

fn command_name(args: &[Vec<u8>]) -> Result<String> {
    let first = args.first().context("请填写 Redis 命令")?;
    anyhow::ensure!(
        !first.is_empty() && first.iter().all(u8::is_ascii_alphabetic),
        "Redis 命令名无效"
    );
    Ok(std::str::from_utf8(first)?.to_ascii_uppercase())
}
pub(super) fn readonly_command(args: &[Vec<u8>]) -> bool {
    let Ok(name) = command_name(args) else {
        return false;
    };
    matches!(
        name.as_str(),
        "PING"
            | "ECHO"
            | "TYPE"
            | "EXISTS"
            | "TTL"
            | "PTTL"
            | "STRLEN"
            | "GETRANGE"
            | "GET"
            | "HGET"
            | "HLEN"
            | "HEXISTS"
            | "HSTRLEN"
            | "LLEN"
            | "LINDEX"
            | "LRANGE"
            | "SCARD"
            | "SISMEMBER"
            | "ZCARD"
            | "ZSCORE"
            | "ZRANK"
            | "ZREVRANK"
            | "ZRANGE"
            | "DBSIZE"
            | "TIME"
    )
}

pub(super) fn validate_command(args: &[Vec<u8>], confirmed: bool) -> Result<String> {
    anyhow::ensure!(args.len() <= MAX_ARGUMENTS, "Redis 命令参数过多");
    let mut bytes = 0usize;
    for arg in args {
        check_bytes(arg)?;
        bytes += arg.len();
    }
    anyhow::ensure!(bytes <= MAX_VALUE_BYTES, "Redis 命令超过 1 MiB 限制");
    let name = command_name(args)?;
    // Deliberately narrow allowlist. Everything else (including AUTH/HELLO,
    // SELECT, transactions, scripts, modules, subscriptions, blocking commands,
    // CLIENT, DEBUG, CONFIG, ACL, KEYS, HGETALL, SMEMBERS, DUMP, FLUSH*) is refused
    // even when confirmed. No credential-bearing command reaches history/output.
    let arity = match name.as_str() {
        "PING" => 1..=2,
        "ECHO" => 2..=2,
        "TYPE" | "TTL" | "PTTL" | "STRLEN" | "GET" | "HLEN" | "LLEN" | "SCARD" | "ZCARD"
        | "PERSIST" => 2..=2,
        "EXISTS" | "DEL" | "UNLINK" => 2..=201,
        "GETRANGE" | "LRANGE" => 4..=4,
        "HGET" | "HEXISTS" | "HSTRLEN" | "LINDEX" | "SISMEMBER" | "ZSCORE" | "ZRANK"
        | "ZREVRANK" | "SETNX" | "EXPIRE" | "PEXPIRE" | "RENAMENX" => 3..=3,
        "ZRANGE" => 4..=5,
        "SET" => 3..=3, // no GET option that could return a huge previous value
        "HSET" | "LSET" | "ZADD" => 4..=4,
        "HDEL" | "RPUSH" | "LPUSH" | "SADD" | "SREM" | "ZREM" => 3..=201,
        "INCR" | "DECR" => 2..=2,
        "INCRBY" | "DECRBY" | "INCRBYFLOAT" => 3..=3,
        "DBSIZE" | "TIME" => 1..=1,
        _ => bail!(
            "此命令不在受限控制台白名单中；连接状态、阻塞、脚本、管理、FLUSH 和无界读取命令均被拒绝"
        ),
    };
    anyhow::ensure!(
        arity.contains(&args.len()),
        "Redis 命令参数数量无效或不支持此选项"
    );
    if matches!(name.as_str(), "LRANGE" | "ZRANGE" | "GETRANGE") {
        let start = integer(&args[2])?;
        let end = integer(&args[3])?;
        let limit = if name == "GETRANGE" {
            MAX_VALUE_BYTES
        } else {
            MAX_COLLECTION_ITEMS
        };
        anyhow::ensure!(
            start >= 0 && end >= start && (end as u64 - start as u64) < limit as u64,
            "只允许有界非负范围（集合最多 200 项，字节最多 1 MiB）"
        );
        if args.len() == 5 {
            anyhow::ensure!(
                args[4].eq_ignore_ascii_case(b"WITHSCORES"),
                "ZRANGE 仅支持 WITHSCORES 选项"
            );
        }
    }
    if name == "LINDEX" {
        integer(&args[2])?;
    }
    if name == "LSET" {
        integer(&args[2])?;
    }
    if matches!(name.as_str(), "EXPIRE" | "PEXPIRE") {
        anyhow::ensure!(integer(&args[2])? > 0, "TTL 必须大于 0；删除请使用 DEL");
    }
    if name == "ZADD" {
        check_score(
            std::str::from_utf8(&args[2])?
                .parse()
                .context("ZADD 分数无效")?,
        )?;
    }
    anyhow::ensure!(
        readonly_command(args) || confirmed,
        "此命令可能修改数据，请确认后再执行"
    );
    Ok(name)
}
fn integer(bytes: &[u8]) -> Result<i64> {
    std::str::from_utf8(bytes)?
        .parse()
        .context("Redis 数字参数无效")
}

fn format_output(value: &Value) -> String {
    fn append(
        value: &Value,
        text: &mut String,
        remaining: &mut usize,
        depth: usize,
        truncated: &mut bool,
    ) {
        use std::fmt::Write;
        if *remaining == 0 || text.len() >= MAX_VALUE_BYTES || depth > 8 {
            *truncated = true;
            return;
        }
        *remaining -= 1;
        match value {
            Value::Nil => text.push_str("(nil)"),
            Value::Okay => text.push_str("OK"),
            Value::Int(value) => {
                let _ = write!(text, "{value}");
            }
            Value::BulkString(value) => {
                // Binary rendering can expand fourfold. UTF-8 text uses the full
                // remaining budget; binary is capped before allocating escapes.
                let available = MAX_VALUE_BYTES - text.len();
                if let Ok(utf8) = std::str::from_utf8(value)
                    && !utf8.chars().any(char::is_control)
                {
                    let mut end = utf8.len().min(available);
                    while !utf8.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.push_str(&utf8[..end]);
                    *truncated |= end != utf8.len();
                } else {
                    let end = value.len().min(available / 4);
                    text.push_str(&display_bytes(&value[..end]));
                    *truncated |= end != value.len();
                }
            }
            Value::SimpleString(value) => text.push_str(value),
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    if *remaining == 0 || text.len() >= MAX_VALUE_BYTES {
                        *truncated = true;
                        break;
                    }
                    let _ = write!(text, "\n{}. ", index + 1);
                    append(value, text, remaining, depth + 1, truncated);
                }
            }
            _ => text.push_str("(不支持的 RESP 值)"),
        }
    }
    let mut text = String::new();
    let mut truncated = false;
    append(value, &mut text, &mut 1000, 0, &mut truncated);
    if truncated || text.len() > MAX_VALUE_BYTES {
        // Truncate at a Unicode boundary and make the limit visible.
        let mut end = text.len().min(MAX_VALUE_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[输出已截断]");
    }
    text
}
