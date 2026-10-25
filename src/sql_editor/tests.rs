use super::*;

#[test]
fn postgres_formatting_preserves_literal_concatenation_newlines() {
    for (sql, expected) in [
        ("SELECT 'foo'\n'bar';", "SELECT 'foo'\n'bar';"),
        ("SELECT '中'\r\n'文';", "SELECT '中'\n'文';"),
        ("SELECT E'foo'\n'bar';", "SELECT E'foo'\n'bar';"),
        ("SELECT 'foo'\n'bar'\n'baz';", "SELECT 'foo'\n'bar'\n'baz';"),
    ] {
        for compact in [false, true] {
            let formatted = format_sql(sql, Engine::Postgres, compact).unwrap();
            assert_eq!(formatted, expected);
            assert_eq!(
                format_sql(&formatted, Engine::Postgres, compact).unwrap(),
                formatted
            );
        }
    }
    assert_eq!(
        format_sql("SELECT 'foo' 'bar';", Engine::Postgres, true).unwrap(),
        "SELECT 'foo' 'bar';"
    );
}

#[test]
fn formatting_preserves_dialect_tokens_and_comment_boundaries() {
    for (engine, sql) in [
        (
            Engine::Postgres,
            "select E'\\\\🦀', $$a  b\nc$$, x::int from public.t -- tail\nwhere id > 1;",
        ),
        (
            Engine::SqlServer,
            "SELECT N'a  b', [中]]文], #temp.x FROM #temp;\nGO\nSELECT 2;",
        ),
        (
            Engine::MySql,
            "select `名``称`, 'a  b', 1--2 from t # comment\nwhere id=1;",
        ),
    ] {
        let tokens = editing_tokens(sql, engine)
            .unwrap()
            .iter()
            .map(|token| token.text.to_owned())
            .collect::<Vec<_>>();
        for compact in [true, false] {
            let formatted = format_sql(sql, engine, compact).unwrap();
            assert_eq!(
                editing_tokens(&formatted, engine)
                    .unwrap()
                    .iter()
                    .map(|token| token.text.to_owned())
                    .collect::<Vec<_>>(),
                tokens
            );
            assert_eq!(format_sql(&formatted, engine, compact).unwrap(), formatted);
        }
    }
    assert!(format_sql("SELECT 'a\\b'", Engine::MySql, false).is_err());
    assert!(format_sql("SELECT /* open", Engine::Postgres, false).is_err());
    for (engine, sql) in [
        (Engine::SqlServer, "SELECT 1;\n/* batch */ GO 2\nSELECT 3;"),
        (
            Engine::MySql,
            "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$\nDELIMITER ;",
        ),
    ] {
        let compact = format_sql(sql, engine, true).unwrap();
        assert_eq!(compact.lines().count(), sql.lines().count());
    }
}

#[test]
fn semantic_completion_resolves_schema_aliases_and_scope() {
    let tables = vec![
        CompletionTable {
            schema: "public".into(),
            name: "users".into(),
            columns: vec!["user_id".into()],
        },
        CompletionTable {
            schema: "sales".into(),
            name: "orders".into(),
            columns: vec!["order_id".into()],
        },
    ];
    let check = |sql: &str, marker: &str| {
        let cursor = sql.find(marker).unwrap() + marker.len();
        semantic_completions(
            sql,
            byte_to_char_index(sql, cursor),
            Engine::Postgres,
            &tables,
            &[],
        )
        .unwrap()
        .1
    };
    assert_eq!(
        check("SELECT u. FROM public.users AS u", "u."),
        [("user_id".into(), CompletionKind::Field)]
    );
    assert_eq!(
        check("SELECT sales. FROM public.users", "sales."),
        [("orders".into(), CompletionKind::Table)]
    );
    assert_eq!(
        check("SELECT u.us FROM public.users u", "u.us"),
        [("user_id".into(), CompletionKind::Field)]
    );
    assert!(check("SELECT 1 FROM public.users u; SELECT u.", "u.").is_empty());
    assert!(check("SELECT (SELECT 1 FROM public.users u), (SELECT u.)", "u.").is_empty());
    assert_eq!(
        check(
            "SELECT (SELECT u. FROM sales.orders u) FROM public.users u",
            "u."
        ),
        [("order_id".into(), CompletionKind::Field)]
    );
    assert_eq!(
        check("SELECT (SELECT u.) FROM public.users u", "u."),
        [("user_id".into(), CompletionKind::Field)]
    );
    assert!(semantic_completions("SELECT 'u.'", 10, Engine::Postgres, &tables, &[]).is_none());
    assert_eq!(
        check("SELECT us FROM public.users", "SELECT us")
            .into_iter()
            .filter(|(_, kind)| *kind != CompletionKind::Keyword)
            .collect::<Vec<_>>(),
        [
            ("user_id".into(), CompletionKind::Field),
            ("users".into(), CompletionKind::Table)
        ]
    );
    let weird = vec![CompletionTable {
        schema: "public".into(),
        name: "users".into(),
        columns: vec!["odd name".into(), "select".into()],
    }];
    let sql = "SELECT u. FROM public.users u";
    assert_eq!(
        semantic_completions(sql, 9, Engine::Postgres, &weird, &[])
            .unwrap()
            .1,
        [
            ("\"odd name\"".into(), CompletionKind::Field),
            ("\"select\"".into(), CompletionKind::Field)
        ]
    );
    assert!(semantic_completions("x.", usize::MAX, Engine::Postgres, &tables, &[]).is_none());
    let cases = vec![CompletionTable {
        schema: "public".into(),
        name: "users".into(),
        columns: vec!["Name".into(), "name".into()],
    }];
    assert_eq!(
        semantic_completions(sql, 9, Engine::Postgres, &cases, &[])
            .unwrap()
            .1,
        [
            ("\"Name\"".into(), CompletionKind::Field),
            ("name".into(), CompletionKind::Field)
        ]
    );
    let sql = "SELECT u. FROM sales.orders u JOIN public.users AS \"U\" ON true";
    assert_eq!(
        semantic_completions(sql, 9, Engine::Postgres, &tables, &[])
            .unwrap()
            .1,
        [("order_id".into(), CompletionKind::Field)]
    );
}

#[test]
fn folds_ignore_literal_parentheses_and_keep_utf8_ranges() {
    let sql = "SELECT '(\n)', (SELECT 中文\n FROM t); /* 中\n文 */";
    let ranges = folding_ranges(sql, Engine::Postgres).unwrap();
    assert_eq!(
        ranges
            .iter()
            .map(|range| &sql[range.clone()])
            .collect::<Vec<_>>(),
        ["(SELECT 中文\n FROM t)", "/* 中\n文 */"]
    );
}

fn statement_at(sql: &str, byte_cursor: usize, engine: Engine) -> Option<&str> {
    let range = current_statement_range(sql, byte_to_char_index(sql, byte_cursor), engine).unwrap();
    range.map(|range| &sql[range])
}

#[test]
fn statement_ranges_respect_unicode_cursor_and_semicolon_boundaries() {
    let sql = "  SELECT '中文🦀';\n SELECT 2;  ";
    let delimiter = sql.find(';').unwrap();
    for engine in Engine::ALL {
        assert_eq!(statement_at(sql, 0, engine), Some("SELECT '中文🦀';"));
        assert_eq!(
            statement_at(sql, delimiter, engine),
            Some("SELECT '中文🦀';")
        );
        assert_eq!(statement_at(sql, delimiter + 1, engine), Some("SELECT 2;"));
        assert_eq!(statement_at(sql, sql.len(), engine), Some("SELECT 2;"));
        assert_eq!(statement_at("SELECT 1", 8, engine), Some("SELECT 1"));
        assert!(current_statement_range(sql, usize::MAX, engine).is_err());
    }
}

#[test]
fn statement_ranges_ignore_quoted_semicolons_and_dialect_comments() {
    let cases = [
        (
            Engine::MySql,
            "SELECT `中;``文`, 'a;''b', \"c;\"; #尾;\nSELECT 2;",
        ),
        (
            Engine::Postgres,
            "SELECT \"中;\"\"文\", 'a;''b', E'\\\'🦀;'; --尾;\nSELECT 2;",
        ),
        (
            Engine::SqlServer,
            "SELECT [中;]]文], N'a;''b' FROM #temp; --尾;\nSELECT 2;",
        ),
    ];
    for (engine, sql) in cases {
        let end = sql.find("; ").unwrap() + 1;
        assert_eq!(
            statement_at(sql, sql.find('中').unwrap(), engine),
            Some(&sql[..end])
        );
        assert!(
            statement_at(sql, sql.len(), engine)
                .unwrap()
                .ends_with("SELECT 2;")
        );
    }
    let sql = "SELECT 1--2;SELECT 3;";
    assert_eq!(statement_at(sql, 0, Engine::MySql), Some("SELECT 1--2;"));
    assert_eq!(statement_at(sql, 0, Engine::Postgres), Some(sql));
    let sql = "SELECT 1;--comment\rSELECT 2;";
    assert_eq!(
        statement_at(sql, sql.len(), Engine::SqlServer),
        Some("--comment\rSELECT 2;")
    );
}

#[test]
fn statement_ranges_support_nested_comments_and_postgres_dollar_strings() {
    let sql = "SELECT /* 外; /* 内; */ 尾 */ $标签$中;🦀$标签$, $$a;b$$, $1; SELECT 2;";
    let end = sql.find("; SELECT").unwrap() + 1;
    assert_eq!(
        statement_at(sql, sql.find('🦀').unwrap(), Engine::Postgres),
        Some(&sql[..end])
    );
    let sql = "SELECT /* 外; /* 内; */ 尾 */ 1; SELECT 2;";
    assert_eq!(
        statement_at(sql, 0, Engine::SqlServer),
        Some("SELECT /* 外; /* 内; */ 尾 */ 1;")
    );
    assert!(current_statement_range(sql, 0, Engine::MySql).is_err());
    let sql = "SELECT a$tag$; SELECT 2;";
    assert_eq!(
        statement_at(sql, 0, Engine::Postgres),
        Some("SELECT a$tag$;")
    );
}

#[test]
fn statement_ranges_do_not_execute_empty_or_comment_only_segments() {
    for engine in Engine::ALL {
        for sql in ["", " \n ", "; ;", "-- 无 SQL;", "/* 只有注释; */"] {
            assert_eq!(statement_at(sql, sql.len(), engine), None);
        }
        let sql = "SELECT 1; -- 只有注释;";
        assert_eq!(statement_at(sql, sql.len(), engine), None);
    }
}

#[test]
fn statement_ranges_refuse_unclosed_quotes_and_ambiguous_session_modes() {
    for engine in Engine::ALL {
        for sql in [
            "SELECT 1; SELECT '未结束;",
            "SELECT \"未结束;",
            "SELECT /* 未结束;",
        ] {
            assert!(
                current_statement_range(sql, 0, engine).is_err(),
                "{engine:?}: {sql}"
            );
        }
    }
    for sql in ["SELECT `未结束;", "SELECT 'a\\b';", "SELECT \"a\\b\";"] {
        assert!(current_statement_range(sql, 0, Engine::MySql).is_err());
    }
    assert!(current_statement_range("SELECT [未结束;", 0, Engine::SqlServer).is_err());
    assert!(current_statement_range("SELECT $tag$未结束;", 0, Engine::Postgres).is_err());
    assert!(current_statement_range("SELECT 'a\\b';", 0, Engine::Postgres).is_err());
    let sql = "SELECT E'a\\b'; SELECT 2;";
    assert_eq!(
        statement_at(sql, 0, Engine::Postgres),
        Some("SELECT E'a\\b';")
    );
    let sql = "SELECT 'a\\b'; SELECT 2;";
    assert_eq!(
        statement_at(sql, 0, Engine::SqlServer),
        Some("SELECT 'a\\b';")
    );
}

#[test]
fn statement_ranges_refuse_procedural_batches_but_preserve_case_expressions() {
    for (engine, sql) in [
        (
            Engine::MySql,
            "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$",
        ),
        (
            Engine::MySql,
            "CREATE DEFINER='u'@'h' PROCEDURE p() BEGIN SELECT 1; END;",
        ),
        (Engine::MySql, "SELECT 1; /*! SELECT 2; */ SELECT 3;"),
        (
            Engine::Postgres,
            "CREATE OR REPLACE FUNCTION f() RETURNS int AS $$SELECT 1;$$ LANGUAGE SQL;",
        ),
        (Engine::Postgres, "DO $$BEGIN SELECT 1; END;$$;"),
        (
            Engine::SqlServer,
            "SELECT 1;\n/* comment */ GO 2 -- batch\nSELECT 2;",
        ),
        (
            Engine::SqlServer,
            "CREATE OR ALTER PROC p AS SELECT 1; SELECT 2;",
        ),
        (Engine::SqlServer, "BEGIN TRY SELECT 1; END TRY;"),
        (Engine::SqlServer, "DECLARE @x int = 1; SELECT @x;"),
        (Engine::SqlServer, "IF 1=1 SELECT 1; ELSE SELECT 2;"),
        (Engine::SqlServer, "SELECT 1\nDECLARE @x int=1; SELECT @x;"),
        (Engine::SqlServer, "SELECT 1\nBEGIN TRY SELECT 2; END TRY;"),
        (Engine::SqlServer, "SELECT 1\nIF 1=1 SELECT 2; SELECT 3;"),
        (
            Engine::SqlServer,
            "SELECT 1\nGOTO finished; SELECT 2; finished: SELECT 3;",
        ),
        (Engine::SqlServer, "SELECT 1; finished: RETURN; SELECT 2;"),
        (
            Engine::Postgres,
            "CREATE RULE r AS ON UPDATE TO t DO ALSO (UPDATE a SET x=1; UPDATE b SET x=2;);",
        ),
        (Engine::Postgres, "SELECT (1; DELETE FROM t;);"),
    ] {
        assert!(
            current_statement_range(sql, 0, engine).is_err(),
            "{engine:?}: {sql}"
        );
    }
    for engine in Engine::ALL {
        let sql = "SELECT CASE WHEN x=1 THEN 'a;b' ELSE 'c' END FROM t; SELECT 2;";
        assert_eq!(
            statement_at(sql, 0, engine),
            Some("SELECT CASE WHEN x=1 THEN 'a;b' ELSE 'c' END FROM t;")
        );
        assert_eq!(
            statement_at("BEGIN TRANSACTION; SELECT 1;", 0, engine),
            Some("BEGIN TRANSACTION;")
        );
        let sql = "SELECT 'GO; DELIMITER; CREATE PROCEDURE';";
        assert_eq!(statement_at(sql, 0, engine), Some(sql));
    }
    let sql = "SELECT IF(x=1, 'a;b', 'c') FROM t; SELECT 2;";
    assert_eq!(
        statement_at(sql, 0, Engine::MySql),
        Some("SELECT IF(x=1, 'a;b', 'c') FROM t;")
    );
}

#[test]
fn completion_matches_words_and_preserves_unicode_cursor() {
    let sql = "🦀 SELECT 中名尾 FROM t";
    let cursor = "🦀 SELECT 中".chars().count();
    let range = completion_word(sql, cursor).unwrap();
    assert_eq!(&sql[range.clone()], "中名尾");
    let choices = completions(
        "中",
        &["中文列".into(), "中文列".into(), "其他".into()],
        &[],
    );
    assert_eq!(choices, [("中文列".into(), CompletionKind::Field)]);
    let mut text = sql.to_owned();
    let cursor = insert_completion(&mut text, range, &choices[0].0).unwrap();
    assert_eq!(text, "🦀 SELECT 中文列 FROM t");
    assert_eq!(cursor, "🦀 SELECT 中文列".chars().count());
    assert_eq!(
        completions("sel", &[], &[]),
        [("SELECT".into(), CompletionKind::Keyword)]
    );
    assert!(completion_word("-- SELECT", 9).is_none());
    assert!(completion_word("SELECT 'abc'", 11).is_none());
    assert!(completion_word("SELECT `abc`", 11).is_none());
    assert!(completion_word("/* abc */", 6).is_none());
    assert!(completion_word("SELECT 123", 10).is_none());
    assert!(completion_word("abc$def", 7).is_some());
    assert!(completion_word("SELECT abc", usize::MAX).is_none());
    assert!(insert_completion(&mut text, 1..3, "x").is_none());
}

#[test]
fn completion_offers_table_names_beside_keywords_and_columns() {
    let tables = [
        "users".to_string(),
        "user_roles".to_string(),
        "orders".to_string(),
    ];
    let choices = completions("use", &[], &tables);
    assert!(choices.contains(&("users".into(), CompletionKind::Table)));
    assert!(choices.contains(&("user_roles".into(), CompletionKind::Table)));
    assert!(!choices.iter().any(|(name, _)| name == "orders"));
    // A table sharing a name with a keyword or column is offered once,
    // as the higher-priority kind.
    let choices = completions("sel", &["seller".into()], &["select_log".into()]);
    assert_eq!(choices[0], ("SELECT".into(), CompletionKind::Keyword));
    let choices = completions("id", &["id_col".into()], &["identities".into()]);
    assert_eq!(
        choices,
        [
            ("id_col".into(), CompletionKind::Field),
            ("identities".into(), CompletionKind::Table),
        ]
    );
    assert!(completions("", &[], &tables).is_empty());
    assert_eq!(CompletionKind::Field.label(), "字段");
    assert_eq!(CompletionKind::Table.label(), "表");
    assert_eq!(CompletionKind::Keyword.label(), "关键字");
}

#[test]
fn unicode_find_replace_and_cursor_indices() {
    let text = "中文 🦀 SELECT 名, select 'Ééİ';";
    let found = find_matches(text, "select", false);
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|range| text.get(range.clone()).is_some()));
    assert_eq!(
        replace_all(text, "select", "查找", false).0,
        "中文 🦀 查找 名, 查找 'Ééİ';"
    );
    assert_eq!(find_matches(text, "é", false).len(), 2);
    assert!(find_matches("İ", "i", false).is_empty());
    assert_eq!(find_matches("İ", "i\u{307}", false), vec![0..2]);
    assert!(find_matches(text, "", true).is_empty());
    let mut value = "中🦀文".to_owned();
    assert!(!replace_match(&mut value, 1..3, "bad"));
    assert!(replace_match(&mut value, 3..7, "好"));
    assert_eq!(value, "中好文");
    assert_eq!(byte_to_char_index("中🦀文", 5), 1);
    assert_eq!(char_to_byte_index("中🦀文", 2), 7);
    assert_eq!(char_to_byte_index("中🦀文", usize::MAX), 10);
}

#[test]
fn replacements_do_not_reprocess_inserted_text() {
    assert_eq!(replace_all("aaa", "aa", "aaaa", true), ("aaaaa".into(), 1));
    assert_eq!(replace_all("中文", "", "x", false), ("中文".into(), 0));
}

#[test]
fn highlights_dialects_without_changing_text_or_splitting_unicode() {
    let inputs = [
        "SELECT `名``称`, [a]]b], \"字段\" FROM t WHERE x = 'it''s 🦀'; --注释\n",
        "SELECT $tag$你好 'x'$tag$, $$🦀$$, 1.2e-3, 0xAB; /* outer /* 中 */ end */",
        "# 中文\nSELECT E'\\中', 'unfinished 🦀",
        "/* 未结束 🦀",
        "[未结束",
        "SELECT 🦀",
        "",
        "'\\🦀'",
    ];
    for input in inputs {
        for dark in [false, true] {
            let job = highlight_sql(input, dark);
            assert_eq!(job.text, input);
            let mut end = 0;
            for section in &job.sections {
                assert_eq!(section.byte_range.start, end);
                assert!(input.get(section.byte_range.clone()).is_some());
                end = section.byte_range.end;
            }
            assert_eq!(end, input.len());
        }
    }
    let job = highlight_sql("SELECT 'str' -- comment", true);
    assert_ne!(job.sections[0].format.color, job.sections[2].format.color);
    assert!(job.sections.last().unwrap().format.italics);
}

#[test]
fn tsv_handles_rectangle_quoting_and_ragged_rows() {
    let rows = vec![
        vec!["id".into(), "名".into(), "tail".into()],
        vec!["1".into(), "a\tb\n\"c\"".into()],
        vec!["2".into()],
    ];
    assert_eq!(
        copy_tsv(&rows, 0..=2, 1..=2),
        "名\ttail\n\"a\tb\n\"\"c\"\"\"\t\n\t"
    );
    assert_eq!(copy_tsv(&rows, 8..=usize::MAX, 0..=2), "");
    assert_eq!(copy_tsv(&rows, 0..=2, 9..=usize::MAX), "");
    assert_eq!(copy_tsv(&[], 0..=0, 0..=0), "");
}
