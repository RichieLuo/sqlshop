use sqlshop::{
    model::{Credentials, Engine, Profile, ResultSet, Table, export_csv},
    storage::Store,
};

#[test]
fn preview_quotes_each_dialect_and_never_interpolates_identifiers_raw() {
    let table = Table {
        schema: "a`\"]".into(),
        name: "users`\"]; DROP TABLE users; --".into(),
        kind: "BASE TABLE".into(),
    };
    assert_eq!(
        Engine::MySql.preview_sql(&table),
        "SELECT * FROM `a``\"]`.`users``\"]; DROP TABLE users; --` LIMIT 200;"
    );
    assert_eq!(
        Engine::Postgres.preview_sql(&table),
        "SELECT * FROM \"a`\"\"]\".\"users`\"\"]; DROP TABLE users; --\" LIMIT 200;"
    );
    assert_eq!(
        Engine::SqlServer.preview_sql(&table),
        "SELECT TOP (200) * FROM [a`\"]]].[users`\"]]; DROP TABLE users; --];"
    );
}

#[test]
fn new_profiles_start_without_tls() {
    let profile = Profile::default();
    assert!(!profile.tls);
    assert!(!profile.trust_certificate);
    assert!(profile.validate().is_ok());
}

#[test]
fn persisted_profiles_preserve_tls_settings() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    for (tls, trust_certificate) in [(false, false), (true, false), (true, true)] {
        let profile = Profile {
            tls,
            trust_certificate,
            ..Profile::default()
        };
        store.save(std::slice::from_ref(&profile)).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded[0].tls, tls);
        assert_eq!(loaded[0].trust_certificate, trust_certificate);
    }
}

#[test]
fn persisted_profiles_round_trip_without_password_fields() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    assert!(store.load().unwrap().is_empty());
    let profile = Profile {
        name: "中文连接".into(),
        ..Profile::default()
    };
    store.save(std::slice::from_ref(&profile)).unwrap();
    let profiles = store.load().unwrap();
    assert_eq!(profiles[0].id, profile.id);
    assert_eq!(profiles[0].name, "中文连接");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("connections.json")).unwrap())
            .unwrap();
    assert!(json[0].get("password").is_none());
    assert!(json[0].get("database").is_none());
    assert!(json[0]["ssh"].get("secret").is_none());
    assert!(json[0]["ssh"].get("password").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(directory.path().join("connections.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    store.save(&[]).unwrap();
    assert!(store.load().unwrap().is_empty());
}

#[test]
fn old_profiles_without_ssh_fields_load_as_direct_connections() {
    let profile = Profile::default();
    let mut json = serde_json::to_value(&profile).unwrap();
    json.as_object_mut().unwrap().remove("ssh");
    let loaded: Profile = serde_json::from_value(json).unwrap();
    assert_eq!(loaded.id, profile.id);
    assert!(!loaded.ssh.enabled);
    assert_eq!(loaded.ssh.port, 22);
    assert!(loaded.validate().is_ok());
}

#[test]
fn database_selection_quotes_names_and_old_profiles_remain_readable() {
    let name = "app`\"]; DROP DATABASE production; --";
    assert_eq!(
        Engine::MySql.select_database_sql(name).unwrap(),
        "USE `app``\"]; DROP DATABASE production; --`;"
    );
    assert_eq!(
        Engine::SqlServer.select_database_sql(name).unwrap(),
        "USE [app`\"]]; DROP DATABASE production; --];"
    );
    assert!(Engine::Postgres.select_database_sql(name).is_none());
    let mut json = serde_json::to_value(Profile::default()).unwrap();
    json["database"] = serde_json::Value::String("legacy-app".into());
    let loaded: Profile = serde_json::from_value(json).unwrap();
    assert_eq!(loaded.database, "legacy-app");
}

#[test]
fn enabled_ssh_requires_host_user_and_a_key_for_private_key_auth() {
    use sqlshop::model::SshAuth;
    let mut profile = Profile::default();
    profile.ssh.enabled = true;
    assert!(profile.validate().is_err());
    profile.ssh.host = "jump.example".into();
    profile.ssh.auth = SshAuth::PrivateKey;
    profile.ssh.private_key.clear();
    assert!(profile.validate().is_err());
    profile.ssh.private_key = "~/.ssh/id_ed25519".into();
    assert!(profile.validate().is_ok());
    profile.ssh.fingerprint = "MD5:invalid".into();
    assert!(profile.validate().is_err());
    // Incomplete inactive settings must not prevent ordinary database connections.
    profile.ssh.enabled = false;
    assert!(profile.validate().is_ok());
}

#[test]
fn corrupted_profiles_are_reported_without_overwriting_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    let path = directory.path().join("connections.json");
    std::fs::write(&path, "corrupted").unwrap();
    assert!(store.load().is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "corrupted");
}

#[test]
fn remembered_credentials_survive_restart_without_plaintext_or_system_keychain() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    let mut profile = Profile::default();
    profile.ssh.enabled = true;
    let credentials = Credentials {
        database_password: "测试数据库密码-123".into(),
        ssh_secret: "测试SSH口令-456".into(),
    };
    assert!(store.credentials(&profile).unwrap().is_none());
    store.save(std::slice::from_ref(&profile)).unwrap();
    store.save_credentials(&profile, &credentials).unwrap();
    let encrypted = std::fs::read(directory.path().join("credentials.enc")).unwrap();
    let key = std::fs::read(directory.path().join("credentials.key")).unwrap();
    assert_eq!(key.len(), 32);
    for secret in [&credentials.database_password, &credentials.ssh_secret] {
        assert!(
            !encrypted
                .windows(secret.len())
                .any(|window| window == secret.as_bytes())
        );
    }
    let restarted = Store {
        directory: directory.path().into(),
    };
    let loaded = restarted
        .credentials(&restarted.load().unwrap()[0])
        .unwrap()
        .unwrap();
    assert_eq!(loaded.database_password, credentials.database_password);
    assert_eq!(loaded.ssh_secret, credentials.ssh_secret);
    // Re-saving uses a fresh nonce and preserves the key needed on restart.
    restarted.save_credentials(&profile, &credentials).unwrap();
    assert_ne!(
        encrypted,
        std::fs::read(directory.path().join("credentials.enc")).unwrap()
    );
    assert_eq!(
        key,
        std::fs::read(directory.path().join("credentials.key")).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["credentials.enc", "credentials.key"] {
            assert_eq!(
                std::fs::metadata(directory.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn forgetting_credentials_preserves_other_connections_and_honors_remember_choices() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    let mut first = Profile::default();
    first.ssh.enabled = true;
    let second = Profile::default();
    let credentials = Credentials {
        database_password: "database-secret".into(),
        ssh_secret: "ssh-secret".into(),
    };
    store.save_credentials(&first, &credentials).unwrap();
    store
        .save_credentials(&second, &Credentials::default())
        .unwrap();
    first.remember_password = false;
    store.save_credentials(&first, &credentials).unwrap();
    let loaded = store.credentials(&first).unwrap().unwrap();
    assert!(loaded.database_password.is_empty());
    assert_eq!(loaded.ssh_secret, "ssh-secret");
    // Re-enabling an option cannot resurrect a forgotten password.
    first.remember_password = true;
    assert!(
        store
            .credentials(&first)
            .unwrap()
            .unwrap()
            .database_password
            .is_empty()
    );
    first.ssh.remember_secret = false;
    store.save_credentials(&first, &credentials).unwrap();
    let loaded = store.credentials(&first).unwrap().unwrap();
    assert_eq!(loaded.database_password, "database-secret");
    assert!(loaded.ssh_secret.is_empty());
    first.remember_password = false;
    store.save_credentials(&first, &credentials).unwrap();
    assert!(store.credentials(&first).unwrap().is_none());
    // An explicitly saved empty password remains distinguishable from no saved entry.
    assert!(
        store
            .credentials(&second)
            .unwrap()
            .unwrap()
            .database_password
            .is_empty()
    );
    store.forget_credentials(first.id).unwrap();
    assert!(store.credentials(&second).unwrap().is_some());
    store.forget_credentials(second.id).unwrap();
    assert!(!directory.path().join("credentials.enc").exists());
}

#[test]
fn corrupted_credentials_or_missing_key_cannot_overwrite_saved_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    let profile = Profile::default();
    let credentials = Credentials {
        database_password: "test-secret".into(),
        ..Default::default()
    };
    store.save_credentials(&profile, &credentials).unwrap();
    let path = directory.path().join("credentials.enc");
    let original = std::fs::read(&path).unwrap();
    let mut corrupted = original.clone();
    *corrupted.last_mut().unwrap() ^= 1;
    std::fs::write(&path, &corrupted).unwrap();
    assert!(store.credentials(&profile).is_err());
    assert!(store.save_credentials(&profile, &credentials).is_err());
    assert!(store.forget_credentials(profile.id).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), corrupted);
    std::fs::write(&path, &original).unwrap();
    std::fs::remove_file(directory.path().join("credentials.key")).unwrap();
    assert!(store.save_credentials(&profile, &credentials).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!directory.path().join("credentials.key").exists());
}

#[test]
fn csv_preserves_quoted_multiline_unicode_empty_and_null_values() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("result.csv");
    let set = ResultSet {
        columns: vec!["text".into(), "empty".into(), "null".into()],
        rows: vec![vec![
            Some("中文,\"quoted\"\nnext line".into()),
            Some(String::new()),
            None,
        ]],
        ..ResultSet::default()
    };
    export_csv(&set, &path).unwrap();
    let mut reader = csv::Reader::from_path(path).unwrap();
    assert_eq!(
        reader.headers().unwrap().iter().collect::<Vec<_>>(),
        ["text", "empty", "null"]
    );
    let row = reader.records().next().unwrap().unwrap();
    assert_eq!(
        row.iter().collect::<Vec<_>>(),
        ["中文,\"quoted\"\nnext line", "", ""]
    );
    let bytes = std::fs::read(directory.path().join("result.csv")).unwrap();
    let (_, parsed) = sqlshop::model::parse_nullable_csv(&bytes).unwrap();
    assert_eq!(
        parsed[0],
        [
            sqlshop::model::InsertValue::Text("中文,\"quoted\"\nnext line".into()),
            sqlshop::model::InsertValue::Text(String::new()),
            sqlshop::model::InsertValue::Null,
        ]
    );
}

#[test]
fn invalid_profiles_do_not_reach_the_network() {
    let mut profile = Profile {
        port: 0,
        ..Profile::default()
    };
    assert!(profile.validate().is_err());
    profile.port = 3306;
    profile.host.clear();
    assert!(profile.validate().is_err());
    profile.host = "localhost".into();
    profile.tls = false;
    profile.trust_certificate = true;
    assert!(profile.validate().is_err());
}

#[test]
fn worker_reports_validation_failure_and_remains_responsive() {
    use sqlshop::worker::{Request, Response, Worker};
    let worker = Worker::start(|| {});
    let profile = Profile {
        name: String::new(),
        ..Profile::default()
    };
    worker
        .send(Request::Test(profile, Default::default()))
        .unwrap();
    assert!(matches!(
        worker
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        Response::Error {
            disconnected: false,
            ..
        }
    ));
    worker.send(Request::Disconnect).unwrap();
    assert!(matches!(
        worker
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        Response::Disconnected
    ));
}

#[test]
fn private_key_paths_allow_extensionless_files_and_report_invalid_selections() {
    use sqlshop::ssh::{expand_path, private_key_path};
    let directory = tempfile::tempdir().unwrap();
    let key = directory.path().join("key with spaces");
    std::fs::write(&key, "test fixture, not a real private key").unwrap();
    assert_eq!(
        private_key_path(&format!("  {}  ", key.display())).unwrap(),
        key
    );
    assert_eq!(expand_path(&format!("  {}  ", key.display())).unwrap(), key);
    let missing = directory.path().join("missing-key");
    let error = private_key_path(missing.to_str().unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("SSH 私钥文件不存在"));
    assert!(error.contains(missing.to_str().unwrap()));
    let public_key = directory.path().join("key.pub");
    std::fs::write(&public_key, "test fixture").unwrap();
    assert!(
        private_key_path(public_key.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("公钥")
    );
    assert!(
        private_key_path(directory.path().to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("必须是文件")
    );
}

#[test]
fn database_certificate_errors_explain_the_correct_connection_settings() {
    use sqlshop::db::connection_error;
    let mut profile = Profile {
        tls: true,
        ..Profile::default()
    };
    let issuer = connection_error(
        &profile,
        anyhow::anyhow!(
            "error communicating with database: invalid peer certificate: UnknownIssuer"
        ),
    );
    let message = format!("{issuer:#}");
    assert!(message.contains("数据库 TLS 证书的签发机构"));
    assert!(message.contains("信任服务器证书"));
    assert!(message.contains("UnknownIssuer"));
    assert!(profile.tls);
    assert!(!profile.trust_certificate);
    profile.ssh.enabled = true;
    let hostname = connection_error(
        &profile,
        anyhow::anyhow!("invalid peer certificate: NotValidForName"),
    );
    assert!(format!("{hostname:#}").contains("127.0.0.1"));
    let auth = connection_error(&profile, anyhow::anyhow!("password authentication failed"));
    assert_eq!(auth.to_string(), "password authentication failed");
}
