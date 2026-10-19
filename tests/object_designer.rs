//! Opt-in DDL integration tests. Only UUID fixtures in the dedicated
//! `sqlshop_test` database are changed. Credentials come from the environment.
use anyhow::{Context, Result, ensure};
use futures_util::FutureExt;
use sqlshop::{
    db::Session,
    model::{Credentials, Engine, Profile},
    object_catalog::{CatalogObject, ObjectKind},
    object_design::{SchemaChange, editable_definition, object_plan},
};
use std::panic::AssertUnwindSafe;

fn config(engine: Engine, prefix: &str) -> (Profile, Credentials) {
    let get = |key: &str| {
        std::env::var(format!("SQLSHOP_TEST_{prefix}_{key}"))
            .unwrap_or_else(|_| panic!("Missing SQLSHOP_TEST_{prefix}_{key}"))
    };
    let database = get("DATABASE");
    assert_eq!(
        database, "sqlshop_test",
        "DDL tests require the dedicated test database"
    );
    (
        Profile {
            engine,
            host: get("HOST"),
            port: get("PORT").parse().expect("Invalid test port"),
            username: get("USER"),
            database,
            tls: false,
            trust_certificate: false,
            remember_password: false,
            ..Profile::default()
        },
        Credentials {
            database_password: get("PASSWORD"),
            ..Default::default()
        },
    )
}

async fn connect(profile: &Profile, credentials: &Credentials) -> Result<Session> {
    let mut session = Session::connect(profile, credentials).await?;
    if session.current_database().await?.as_deref() != Some("sqlshop_test") {
        session.select_database("sqlshop_test").await?;
    }
    ensure!(
        session.current_database().await?.as_deref() == Some("sqlshop_test"),
        "Unexpected database; no fixture SQL was issued"
    );
    Ok(session)
}

struct Fixtures {
    schema: String,
    rename: String,
    view: String,
    routine: String,
    sequence: String,
}
impl Fixtures {
    fn new(engine: Engine) -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        Self {
            schema: if engine == Engine::MySql {
                "sqlshop_test".into()
            } else {
                format!("od_{id}")
            },
            rename: format!("odr_{id}"),
            view: format!("odv_{id}"),
            routine: format!("odf_{id}"),
            sequence: format!("ods_{id}"),
        }
    }
    fn object(&self, name: &str, kind: ObjectKind) -> CatalogObject {
        CatalogObject {
            schema: self.schema.clone(),
            name: name.into(),
            kind,
            owner: None,
            comment: None,
            estimated_rows: None,
            size_bytes: None,
            created_at: None,
            updated_at: None,
            engine: None,
            signature: None,
            parent: None,
        }
    }
    fn qualified(&self, engine: Engine, name: &str) -> String {
        format!("{}.{}", engine.quote(&self.schema), engine.quote(name))
    }
    fn routine_kind(engine: Engine) -> ObjectKind {
        if engine == Engine::SqlServer {
            ObjectKind::Procedure
        } else {
            ObjectKind::Function
        }
    }
    fn routine_ddl(&self, engine: Engine, value: i32) -> String {
        let name = self.qualified(engine, &self.routine);
        match engine {
            Engine::MySql => {
                format!("CREATE FUNCTION {name}() RETURNS INTEGER DETERMINISTIC RETURN {value};")
            }
            Engine::Postgres => format!(
                "CREATE FUNCTION {name}() RETURNS integer LANGUAGE sql AS $$ SELECT {value}; $$;"
            ),
            Engine::SqlServer => format!("CREATE PROCEDURE {name} AS SELECT {value} AS marker;"),
        }
    }
    async fn cleanup(&self, profile: &Profile, credentials: &Credentials) -> Result<()> {
        let engine = profile.engine;
        let mut cleanup = connect(profile, credentials).await?;
        let routine_type = if engine == Engine::SqlServer {
            "PROCEDURE"
        } else {
            "FUNCTION"
        };
        let routine_args = if engine == Engine::Postgres { "()" } else { "" };
        let mut statements = vec![
            format!(
                "DROP VIEW IF EXISTS {};",
                self.qualified(engine, &self.view)
            ),
            format!(
                "DROP {routine_type} IF EXISTS {}{routine_args};",
                self.qualified(engine, &self.routine)
            ),
        ];
        if engine != Engine::MySql {
            statements.push(format!(
                "DROP SEQUENCE IF EXISTS {};",
                self.qualified(engine, &self.sequence)
            ));
            statements.push(format!(
                "DROP SCHEMA IF EXISTS {};",
                engine.quote(&self.schema)
            ));
            statements.push(format!(
                "DROP SCHEMA IF EXISTS {};",
                engine.quote(&self.rename)
            ));
        }
        // A failure must not prevent attempting the remaining fixture cleanup.
        let mut errors = Vec::new();
        for statement in statements {
            if let Err(error) = cleanup.query(&statement).await {
                errors.push(format!("fixture cleanup failed: {error:#}"));
            }
        }
        ensure!(errors.is_empty(), "{}", errors.join("\n"));
        Ok(())
    }
}

async fn object(session: &mut Session, fixture: &CatalogObject) -> Result<CatalogObject> {
    let catalog = session.object_catalog().await?;
    ensure!(
        !catalog.truncated && !catalog.unavailable_kinds.contains(&fixture.kind),
        "Fixture object catalog was unavailable"
    );
    catalog
        .objects
        .into_iter()
        .find(|o| o.kind == fixture.kind && o.schema == fixture.schema && o.name == fixture.name)
        .context("Created fixture missing from object catalog")
}
async fn scalar(session: &mut Session, sql: &str) -> Result<String> {
    let output = session.query(sql).await?;
    output
        .sets
        .into_iter()
        .find_map(|set| {
            set.rows
                .into_iter()
                .find_map(|row| row.into_iter().next().flatten())
        })
        .context("Expected one fixture scalar result")
}
async fn apply(
    session: &mut Session,
    engine: Engine,
    object: &CatalogObject,
    original: Option<&str>,
    ddl: &str,
) -> Result<()> {
    let preview = object_plan(engine, object, ddl, original.is_some())?;
    session
        .apply_object_design(object, original, ddl, &preview)
        .await?;
    Ok(())
}

async fn exercise(session: &mut Session, engine: Engine, fixtures: &Fixtures) -> Result<()> {
    if engine != Engine::MySql {
        let create = SchemaChange::Create {
            name: fixtures.schema.clone(),
            owner: String::new(),
        };
        session
            .apply_schema_design(&create, &[], &create.sql(engine)?)
            .await?;
        let schemas = session.design_schemas().await?;
        let schema = schemas
            .iter()
            .find(|s| s.name == fixtures.schema)
            .context("Created schema missing")?;
        ensure!(
            !schema.owner.is_empty() && !schema.identity.is_empty(),
            "Schema identity/owner was not returned"
        );
        let owner = SchemaChange::Owner {
            name: fixtures.schema.clone(),
            owner: schema.owner.clone(),
        };
        session
            .apply_schema_design(&owner, &schemas, &owner.sql(engine)?)
            .await?;
        let mut stale = schemas.clone();
        stale
            .iter_mut()
            .find(|s| s.name == fixtures.schema)
            .unwrap()
            .identity
            .push_str("_stale");
        assert!(
            session
                .apply_schema_design(&owner, &stale, &owner.sql(engine)?)
                .await
                .is_err(),
            "Schema identity change must reject mutation"
        );
        if engine == Engine::Postgres {
            let rename = SchemaChange::Rename {
                name: fixtures.schema.clone(),
                to: fixtures.rename.clone(),
            };
            let schemas = session.design_schemas().await?;
            session
                .apply_schema_design(&rename, &schemas, &rename.sql(engine)?)
                .await?;
            let back = SchemaChange::Rename {
                name: fixtures.rename.clone(),
                to: fixtures.schema.clone(),
            };
            let schemas = session.design_schemas().await?;
            session
                .apply_schema_design(&back, &schemas, &back.sql(engine)?)
                .await?;
        }
    }

    let view = fixtures.object(&fixtures.view, ObjectKind::View);
    let name = fixtures.qualified(engine, &fixtures.view);
    let ddl = format!("CREATE VIEW {name} AS SELECT 1 AS marker;");
    assert!(
        session
            .apply_object_design(&view, None, &ddl, &["SELECT 1;".into()])
            .await
            .is_err(),
        "Execution must reject a changed preview"
    );
    apply(session, engine, &view, None, &ddl).await?;
    let view = object(session, &view).await?;
    let original = session.object_definition(&view).await?;
    let canonical = editable_definition(engine, &view, &original)?;
    // Actual catalog DDL (including unqualified module names/modifiers) must be editable.
    object_plan(engine, &view, &canonical, true)?;
    let edited = format!("CREATE VIEW {name} AS SELECT 2 AS marker;");
    apply(session, engine, &view, Some(&original), &edited).await?;
    assert_eq!(
        scalar(session, &format!("SELECT marker FROM {name}")).await?,
        "2"
    );
    let stale_edit = format!("CREATE VIEW {name} AS SELECT 3 AS marker;");
    assert!(
        apply(session, engine, &view, Some(&original), &stale_edit)
            .await
            .is_err(),
        "An old definition must not overwrite a newer view"
    );
    assert_eq!(
        scalar(session, &format!("SELECT marker FROM {name}")).await?,
        "2"
    );
    let current = session.object_definition(&view).await?;
    let invalid = format!("CREATE VIEW {name} AS SELECT missing_designer_fixture_column;");
    assert!(
        apply(session, engine, &view, Some(&current), &invalid)
            .await
            .is_err(),
        "Invalid replacement must surface a server error"
    );
    assert_eq!(
        session.object_definition(&view).await?,
        current,
        "Failed view edit must preserve the server definition"
    );
    assert_eq!(
        scalar(session, &format!("SELECT marker FROM {name}")).await?,
        "2"
    );
    if engine != Engine::MySql {
        let drop_schema = SchemaChange::Drop {
            name: fixtures.schema.clone(),
        };
        let schemas = session.design_schemas().await?;
        assert!(
            session
                .apply_schema_design(&drop_schema, &schemas, &drop_schema.sql(engine)?)
                .await
                .is_err(),
            "Nonempty schema deletion must be rejected without CASCADE"
        );
        assert_eq!(
            scalar(session, &format!("SELECT marker FROM {name}")).await?,
            "2"
        );
    }

    let routine = fixtures.object(&fixtures.routine, Fixtures::routine_kind(engine));
    apply(
        session,
        engine,
        &routine,
        None,
        &fixtures.routine_ddl(engine, 1),
    )
    .await?;
    let routine = object(session, &routine).await?;
    let original = session.object_definition(&routine).await?;
    let editable = editable_definition(engine, &routine, &original)?;
    object_plan(engine, &routine, &editable, true)?;
    apply(
        session,
        engine,
        &routine,
        Some(&original),
        &fixtures.routine_ddl(engine, 2),
    )
    .await?;
    let routine_name = fixtures.qualified(engine, &fixtures.routine);
    let invoke = if engine == Engine::SqlServer {
        format!("EXEC {routine_name};")
    } else {
        format!("SELECT {routine_name}();")
    };
    assert_eq!(scalar(session, &invoke).await?, "2");
    assert!(
        apply(
            session,
            engine,
            &routine,
            Some(&original),
            &fixtures.routine_ddl(engine, 3)
        )
        .await
        .is_err(),
        "Stale routine snapshot must reject replacement"
    );
    assert_eq!(scalar(session, &invoke).await?, "2");
    if engine == Engine::Postgres {
        let current = session.object_definition(&routine).await?;
        let changed_identity = format!(
            "CREATE FUNCTION {routine_name}(arg integer) RETURNS integer LANGUAGE sql AS $$ SELECT arg; $$;"
        );
        assert!(
            apply(session, engine, &routine, Some(&current), &changed_identity)
                .await
                .is_err(),
            "Changing overload arguments must reject editing"
        );
        let catalog = session.object_catalog().await?;
        assert_eq!(
            catalog
                .objects
                .iter()
                .filter(|o| o.schema == routine.schema
                    && o.name == routine.name
                    && o.kind == routine.kind)
                .count(),
            1,
            "Rejected argument edit must not create another overload"
        );
    }

    if engine != Engine::MySql {
        let sequence = fixtures.object(&fixtures.sequence, ObjectKind::Sequence);
        let sequence_name = fixtures.qualified(engine, &fixtures.sequence);
        apply(
            session,
            engine,
            &sequence,
            None,
            &format!("CREATE SEQUENCE {sequence_name} START WITH 41 INCREMENT BY 1;"),
        )
        .await?;
        let sequence = object(session, &sequence).await?;
        let original = session.object_definition(&sequence).await?;
        let next = if engine == Engine::Postgres {
            format!(
                "SELECT nextval('{}.{}');",
                engine.quote(&fixtures.schema),
                engine.quote(&fixtures.sequence)
            )
        } else {
            format!("SELECT NEXT VALUE FOR {sequence_name};")
        };
        assert_eq!(scalar(session, &next).await?, "41");
        apply(
            session,
            engine,
            &sequence,
            Some(&original),
            &format!("ALTER SEQUENCE {sequence_name} INCREMENT BY 2;"),
        )
        .await?;
        assert_eq!(
            scalar(session, &next).await?,
            "43",
            "Changing increment must not restart the sequence"
        );
        session
            .query(&format!("DROP SEQUENCE {sequence_name};"))
            .await?;
        session.query(&format!("DROP VIEW {name};")).await?;
        session
            .query(&if engine == Engine::Postgres {
                format!("DROP FUNCTION {routine_name}();")
            } else {
                format!("DROP PROCEDURE {routine_name};")
            })
            .await?;
        let schemas = session.design_schemas().await?;
        let drop_schema = SchemaChange::Drop {
            name: fixtures.schema.clone(),
        };
        session
            .apply_schema_design(&drop_schema, &schemas, &drop_schema.sql(engine)?)
            .await?;
        assert!(
            !session
                .design_schemas()
                .await?
                .iter()
                .any(|s| s.name == fixtures.schema),
            "Empty schema deletion must remove its catalog entry"
        );
    }
    Ok(())
}

fn run(engine: Engine, prefix: &str) {
    let (profile, credentials) = config(engine, prefix);
    let fixtures = Fixtures::new(engine);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let result = AssertUnwindSafe(async {
            let mut session = connect(&profile, &credentials).await?;
            exercise(&mut session, engine, &fixtures).await
        })
        .catch_unwind()
        .await;
        // Fresh connection also handles a failed/aborted driver connection.
        let cleanup = fixtures.cleanup(&profile, &credentials).await;
        match result {
            Ok(Ok(())) => cleanup.expect("Fixture cleanup failed"),
            Ok(Err(error)) => {
                panic!("Object designer integration failed: {error:#}; cleanup result: {cleanup:?}")
            }
            Err(panic) => {
                if cleanup.is_err() {
                    eprintln!(
                        "Fixture cleanup also failed; inspect the UUID test schema before retrying"
                    );
                }
                std::panic::resume_unwind(panic);
            }
        }
    });
}

#[test]
#[ignore = "requires dedicated MySQL Docker database and SQLSHOP_TEST_MYSQL_* credentials"]
fn mysql_object_designer_round_trip() {
    run(Engine::MySql, "MYSQL");
}
#[test]
#[ignore = "requires dedicated PostgreSQL Docker database and SQLSHOP_TEST_PG_* credentials"]
fn postgres_object_designer_round_trip() {
    run(Engine::Postgres, "PG");
}
#[test]
#[ignore = "requires dedicated SQL Server Docker database and SQLSHOP_TEST_MSSQL_* credentials"]
fn sqlserver_object_designer_round_trip() {
    run(Engine::SqlServer, "MSSQL");
}
