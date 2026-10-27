//! Opt-in round trips against the existing dedicated sqlshop_test containers.
use sqlshop::{
    db::{
        ConstraintDesign, ConstraintKind, DesignSnapshot, DesignerOperation, ReferenceAction,
        Session, StructureChange, structure_change_sql,
    },
    model::{Credentials, Engine, Profile, Table},
    worker::{Request, Response, Worker},
    workspace::QueryTarget,
};
use std::time::{Duration, Instant};
use uuid::Uuid;
fn config(engine: Engine, prefix: &str) -> (Profile, Credentials) {
    let get = |k: &str| std::env::var(format!("SQLSHOP_TEST_{prefix}_{k}")).unwrap();
    let database = get("DATABASE");
    assert_eq!(database, "sqlshop_test", "requires dedicated test database");
    (
        Profile {
            engine,
            host: get("HOST"),
            port: get("PORT").parse().unwrap(),
            username: get("USER"),
            database,
            tls: false,
            ..Default::default()
        },
        Credentials {
            database_password: get("PASSWORD"),
            ..Default::default()
        },
    )
}
async fn connect(profile: &Profile, credentials: &Credentials) -> Session {
    let mut s = Session::connect(profile, credentials).await.unwrap();
    s.select_database("sqlshop_test").await.unwrap();
    s
}
async fn apply(s: &mut Session, engine: Engine, t: &Table, op: DesignerOperation) {
    let snapshot = DesignSnapshot::from(&s.table_structure(t).await.unwrap());
    let c = StructureChange::Designer {
        operation: Box::new(op),
        snapshot,
    };
    let sql = structure_change_sql(engine, t, &c).unwrap();
    s.change_structure("sqlshop_test", t, &c, &sql)
        .await
        .unwrap();
}
fn constraint(name: &str, kind: ConstraintKind, columns: &[&str]) -> ConstraintDesign {
    ConstraintDesign {
        name: name.into(),
        kind,
        columns: columns.iter().map(|s| (*s).into()).collect(),
        expression: String::new(),
        referenced_schema: String::new(),
        referenced_table: String::new(),
        referenced_columns: vec![],
        on_delete: ReferenceAction::NoAction,
        on_update: ReferenceAction::NoAction,
        deferrable: false,
        initially_deferred: false,
    }
}
fn exercise(engine: Engine, prefix: &str) {
    let (profile, credentials) = config(engine, prefix);
    tokio::runtime::Runtime::new().unwrap().block_on(async{
 let suffix=&Uuid::new_v4().simple().to_string()[..12];let t=Table{schema:match engine{Engine::MySql=>"sqlshop_test",Engine::Postgres=>"public",Engine::SqlServer=>"dbo"}.into(),name:format!("designer_{suffix}"),kind:"BASE TABLE".into()};let q=format!("{}.{}",engine.quote(&t.schema),engine.quote(&t.name));let mut seed=connect(&profile,&credentials).await;seed.query(&format!("CREATE TABLE {q} (id INT NOT NULL, parent_id INT NULL, label VARCHAR(40) NULL, amount INT NULL)")).await.unwrap();drop(seed);
 let result=std::panic::AssertUnwindSafe(async{
 let mut s=connect(&profile,&credentials).await;
 apply(&mut s,engine,&t,DesignerOperation::RenameColumn{old:"label".into(),new:"display_name".into()}).await;
 apply(&mut s,engine,&t,DesignerOperation::SetDefault{column:"amount".into(),expression:Some("7".into())}).await;
 apply(&mut s,engine,&t,DesignerOperation::SetDefault{column:"amount".into(),expression:Some("8".into())}).await;
 apply(&mut s,engine,&t,DesignerOperation::AlterType{column:"display_name".into(),data_type:"VARCHAR(80)".into(),using:None}).await;
 let pk=format!("pk_{suffix}");apply(&mut s,engine,&t,DesignerOperation::PutConstraint{previous:None,definition:constraint(&pk,ConstraintKind::PrimaryKey,&["id"])}).await;
 let uk=format!("uk_{suffix}");apply(&mut s,engine,&t,DesignerOperation::PutConstraint{previous:None,definition:constraint(&uk,ConstraintKind::Unique,&["display_name"])}).await;
 let ck=format!("ck_{suffix}");let mut check=constraint(&ck,ConstraintKind::Check,&[]);check.expression="amount >= 0".into();apply(&mut s,engine,&t,DesignerOperation::PutConstraint{previous:None,definition:check.clone()}).await;
 check.expression="amount >= -1".into();apply(&mut s,engine,&t,DesignerOperation::PutConstraint{previous:Some((ck.clone(),ConstraintKind::Check)),definition:check}).await;
 let fk=format!("fk_{suffix}");let mut foreign=constraint(&fk,ConstraintKind::ForeignKey,&["parent_id"]);foreign.referenced_schema=t.schema.clone();foreign.referenced_table=t.name.clone();foreign.referenced_columns=vec!["id".into()];apply(&mut s,engine,&t,DesignerOperation::PutConstraint{previous:None,definition:foreign}).await;
 let current=s.table_structure(&t).await.unwrap();assert!(current.constraints.rows.iter().any(|r|r.first().and_then(Option::as_deref)==Some(&fk)));assert!(current.columns.rows.iter().any(|r|r.get(1).and_then(Option::as_deref)==Some("display_name")));
 let stale=StructureChange::Designer{operation:Box::new(DesignerOperation::SetDefault{column:"amount".into(),expression:Some("9".into())}),snapshot:DesignSnapshot::from(&current)};let preview=structure_change_sql(engine,&t,&stale).unwrap();
 apply(&mut s,engine,&t,DesignerOperation::SetDefault{column:"amount".into(),expression:None}).await;assert!(s.change_structure("sqlshop_test",&t,&stale,&preview).await.is_err());
 for (name,kind) in [(fk,ConstraintKind::ForeignKey),(ck,ConstraintKind::Check),(uk,ConstraintKind::Unique)]{apply(&mut s,engine,&t,DesignerOperation::DropConstraint{name,kind}).await;}
 apply(&mut s,engine,&t,DesignerOperation::AddGenerated{column:"computed_amount".into(),data_type:"INT".into(),expression:"amount * 2".into(),stored:true}).await;
 let st=s.table_structure(&t).await.unwrap();assert!(st.columns.rows.iter().any(|r|r.get(1).and_then(Option::as_deref)==Some("computed_amount")));
 }).catch_unwind().await;
 let mut cleanup=connect(&profile,&credentials).await;cleanup.query(&format!("DROP TABLE {q}")).await.unwrap();if let Err(panic)=result{std::panic::resume_unwind(panic);}
});
}
use futures_util::FutureExt;
#[test]
#[ignore = "dedicated local containers"]
fn mysql_table_designer() {
    exercise(Engine::MySql, "MYSQL")
}
#[test]
#[ignore = "dedicated local containers"]
fn postgres_table_designer() {
    exercise(Engine::Postgres, "PG")
}
#[test]
#[ignore = "dedicated local containers"]
fn sqlserver_table_designer() {
    exercise(Engine::SqlServer, "MSSQL")
}

fn exercise_table_rename(engine: Engine, prefix: &str) {
    let (profile, credentials) = config(engine, prefix);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let suffix = &Uuid::new_v4().simple().to_string()[..12];
        let table = Table {
            schema: match engine { Engine::MySql => "sqlshop_test", Engine::Postgres => "public", Engine::SqlServer => "dbo" }.into(),
            name: format!("rename_{suffix}"), kind: "BASE TABLE".into(),
        };
        let renamed = Table { name: format!("renamed_{suffix}`\"]'"), ..table.clone() };
        let occupied = Table { name: format!("occupied_{suffix}"), ..table.clone() };
        let view = Table { name: format!("view_{suffix}"), kind: "VIEW".into(), ..table.clone() };
        let qualified = |table: &Table| format!("{}.{}", engine.quote(&table.schema), engine.quote(&table.name));
        let mut seed = connect(&profile, &credentials).await;
        seed.query(&format!("CREATE TABLE {} (id INT PRIMARY KEY, note VARCHAR(40)); INSERT INTO {} VALUES (7, 'preserved'); CREATE TABLE {} (id INT);", qualified(&table), qualified(&table), qualified(&occupied))).await.unwrap();
        seed.query(&format!("CREATE VIEW {} AS SELECT id FROM {}", qualified(&view), qualified(&table))).await.unwrap();
        drop(seed);
        let result = std::panic::AssertUnwindSafe(async {
            let mut session = connect(&profile, &credentials).await;
            assert!(session.rename_table("wrong_database", &table, &renamed.name).await.is_err());
            assert!(session.rename_table("sqlshop_test", &table, &occupied.name).await.is_err());
            assert!(session.rename_table("sqlshop_test", &view, "not_a_table").await.is_err());
            let disguised_view = Table { kind: "BASE TABLE".into(), ..view.clone() };
            assert!(session.rename_table("sqlshop_test", &disguised_view, "not_a_table").await.is_err());
            session.rename_table("sqlshop_test", &table, &renamed.name).await.unwrap();
            let tables = session.tables().await.unwrap();
            assert!(tables.contains(&renamed));
            assert!(!tables.contains(&table));
            assert!(session.rename_table("sqlshop_test", &table, "missing_source").await.is_err());
            let output = session.query(&format!("SELECT id, note FROM {}", qualified(&renamed))).await.unwrap();
            assert_eq!(output.sets[0].rows[0], vec![Some("7".into()), Some("preserved".into())]);
        }).catch_unwind().await;
        let mut cleanup = connect(&profile, &credentials).await;
        cleanup.query(&format!("DROP VIEW {}", qualified(&view))).await.unwrap();
        for candidate in [&table, &renamed, &occupied] {
            if cleanup.tables().await.unwrap().contains(candidate) {
                cleanup.query(&format!("DROP TABLE {}", qualified(candidate))).await.unwrap();
            }
        }
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    });
}

#[test]
#[ignore = "dedicated local containers"]
fn mysql_table_rename() {
    exercise_table_rename(Engine::MySql, "MYSQL");
}

#[test]
#[ignore = "dedicated local containers"]
fn postgres_table_rename() {
    exercise_table_rename(Engine::Postgres, "PG");
}

#[test]
#[ignore = "dedicated local containers"]
fn sqlserver_table_rename() {
    exercise_table_rename(Engine::SqlServer, "MSSQL");
}
fn response(w: &Worker) -> Response {
    w.receiver
        .recv_timeout(Duration::from_secs(20))
        .expect("worker response")
}
fn cancellation(engine: Engine, prefix: &str, last: bool) {
    let (profile, credentials) = config(engine, prefix);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let suffix = &Uuid::new_v4().simple().to_string()[..12];
    let t = Table {
        schema: match engine {
            Engine::MySql => "sqlshop_test",
            Engine::Postgres => "public",
            Engine::SqlServer => "dbo",
        }
        .into(),
        name: format!("cancel_read_{suffix}"),
        kind: "BASE TABLE".into(),
    };
    let q = format!("{}.{}", engine.quote(&t.schema), engine.quote(&t.name));
    let mut lock = runtime.block_on(connect(&profile, &credentials));
    runtime.block_on(lock.query(&format!("CREATE TABLE {q} (id INT NOT NULL PRIMARY KEY, label VARCHAR(40)); INSERT INTO {q} VALUES (1, 'ready')"))).unwrap();
    let w = Worker::start(|| {});
    w.send(Request::Connect(profile.clone(), credentials.clone()))
        .unwrap();
    assert!(matches!(response(&w), Response::Connected(_)));
    w.send(Request::SelectDatabase("sqlshop_test".into()))
        .unwrap();
    assert!(matches!(response(&w), Response::DatabaseSelected(_)));
    let lock_sql = match engine {
        Engine::MySql => format!("LOCK TABLES {q} WRITE"),
        Engine::Postgres => format!("BEGIN; LOCK TABLE {q} IN ACCESS EXCLUSIVE MODE"),
        Engine::SqlServer => format!("BEGIN TRANSACTION; UPDATE {q} SET label='locked' WHERE id=1"),
    };
    runtime.block_on(lock.query(&lock_sql)).unwrap();
    let tab = Uuid::new_v4();
    let target = QueryTarget {
        connection: profile.id,
        database: Some("sqlshop_test".into()),
        schema: Some(t.schema.clone()),
    };
    let request = |last| {
        if last {
            Request::ReadLastPage {
                tab,
                target: target.clone(),
                table: t.clone(),
                options: Default::default(),
            }
        } else {
            Request::ReadPage {
                tab,
                target: target.clone(),
                table: t.clone(),
                options: Default::default(),
            }
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        w.send(request(last)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let execution = loop {
            if let Some(read) = w.table_read_execution(tab)
                && read.cancellable
            {
                break read.execution;
            }
            assert!(Instant::now() < deadline, "read did not arm cancellation");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(w.cancel_table_read(tab, Uuid::new_v4()).is_err());
        std::thread::sleep(Duration::from_millis(200));
        w.cancel_table_read(tab, execution).unwrap();
        let stopped = response(&w);
        match stopped {
            Response::OperationError {
                message,
                disconnected: false,
                ..
            } => assert!(message.contains("停止"), "{message}"),
            Response::OperationError {
                message,
                disconnected,
                ..
            }
            | Response::Error {
                message,
                disconnected,
            } => {
                panic!("read cancellation failed: disconnected={disconnected}, {message}")
            }
            other => panic!(
                "expected confirmed or between-request read cancellation, got {:?}",
                std::mem::discriminant(&other)
            ),
        };
        assert!(w.table_read_execution(tab).is_none());
        assert!(w.cancel_table_read(tab, execution).is_err());
    }));
    runtime
        .block_on(lock.query(if engine == Engine::MySql {
            "UNLOCK TABLES"
        } else {
            "ROLLBACK"
        }))
        .unwrap();
    if result.is_ok() {
        w.send(request(false)).unwrap();
        assert!(
            matches!(response(&w), Response::TablePage { .. }),
            "session must be reusable after read cancellation"
        );
    }
    runtime
        .block_on(lock.query(&format!("DROP TABLE {q}")))
        .unwrap();
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}
#[test]
#[ignore = "dedicated local containers"]
fn mysql_page_cancellation() {
    cancellation(Engine::MySql, "MYSQL", false)
}
#[test]
#[ignore = "dedicated local containers"]
fn postgres_page_cancellation() {
    cancellation(Engine::Postgres, "PG", false)
}
#[test]
#[ignore = "dedicated local containers"]
fn sqlserver_page_cancellation() {
    cancellation(Engine::SqlServer, "MSSQL", false)
}
#[test]
#[ignore = "dedicated local containers"]
fn mysql_last_page_cancellation() {
    cancellation(Engine::MySql, "MYSQL", true)
}
#[test]
#[ignore = "dedicated local containers"]
fn postgres_last_page_cancellation() {
    cancellation(Engine::Postgres, "PG", true)
}
#[test]
#[ignore = "dedicated local containers"]
fn sqlserver_last_page_cancellation() {
    cancellation(Engine::SqlServer, "MSSQL", true)
}
