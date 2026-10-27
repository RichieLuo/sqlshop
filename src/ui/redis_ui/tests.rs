use super::*;

#[test]
fn key_tree_scrollbar_stays_at_resized_panel_edge_for_short_and_long_keys() {
    for dark in [false, true] {
        for long_keys in [false, true] {
            let context = egui::Context::default();
            context.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut doc = RedisDocument::fixture(&context);
            doc.keys = (0..200)
                .map(|index| {
                    if long_keys {
                        format!("{index:04}-{}", "long-key-".repeat(40)).into_bytes()
                    } else {
                        format!("key-{index:04}").into_bytes()
                    }
                })
                .collect();
            doc.rebuild_tree();
            let mut time = 0.0;
            let mut frame = |events| {
                time += 0.1;
                let mut geometry = (egui::Rect::NOTHING, egui::Rect::NOTHING);
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 700.0),
                        )),
                        events,
                        time: Some(time),
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| {
                            geometry = doc.key_value_workspace(ui, 600.0);
                        });
                    },
                );
                geometry
            };
            frame(vec![]);
            let (divider, viewport) = frame(vec![]);
            let gap = divider.center().x - viewport.right();
            assert!(
                (7.0..=28.0).contains(&gap),
                "scrollbar must follow panel edge, gap={gap}"
            );
            let start = divider.center();
            let pointer = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![egui::Event::PointerMoved(start), pointer(start, true)]);
            let moved = start + egui::vec2(230.0, 0.0);
            frame(vec![egui::Event::PointerMoved(moved)]);
            frame(vec![egui::Event::PointerMoved(moved)]);
            frame(vec![pointer(moved, false)]);
            let (resized, viewport) = frame(vec![]);
            assert!(resized.center().x > divider.center().x + 180.0);
            let gap = resized.center().x - viewport.right();
            assert!(
                (7.0..=28.0).contains(&gap),
                "resized scrollbar drifted into pane, gap={gap}"
            );
            assert!(
                viewport.width() > 500.0,
                "scroll viewport must fill widened pane"
            );
        }
    }
}

#[test]
fn dragging_key_value_separator_resizes_and_preserves_width() {
    for dark in [false, true] {
        let context = egui::Context::default();
        context.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut doc = RedisDocument::fixture(&context);
        let mut time = 0.0;
        let mut frame = |events| {
            time += 0.1;
            let mut rect = egui::Rect::NOTHING;
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 700.0),
                    )),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        rect = doc.key_value_workspace(ui, 600.0).0;
                    });
                },
            );
            rect
        };
        let initial = frame(Vec::new());
        let start = initial.center();
        let pointer = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(start), pointer(start, true)]);
        let moved = start + egui::vec2(160.0, 0.0);
        frame(vec![egui::Event::PointerMoved(moved)]);
        let grown = frame(vec![egui::Event::PointerMoved(moved)]);
        assert!(
            grown.center().x > initial.center().x + 100.0,
            "separator must be draggable"
        );
        frame(vec![pointer(moved, false)]);
        let persisted = frame(Vec::new());
        assert!((persisted.center().x - grown.center().x).abs() < 2.0);
        assert!(
            persisted.center().x <= 760.0,
            "value pane must retain usable width"
        );
    }
}

#[test]
fn namespace_folder_context_menu_requests_preview_not_immediate_delete() {
    let context = egui::Context::default();
    let mut tree = KeyTree::default();
    tree.insert(b"app:users:1");
    let mut requested = None;
    let mut time = 0.0;
    let mut frame = |events| {
        time += 0.1;
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                time: Some(time),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    draw_tree(
                        ui,
                        &tree,
                        None,
                        &mut Vec::new(),
                        &mut None,
                        &mut requested,
                        0,
                    );
                });
            },
        )
    };
    fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text().contains(label) => {
                    Some(text.pos + egui::vec2(10.0, 5.0))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing label: {label}"))
    }
    let output = frame(vec![]);
    let position = text_position(&output, "app  (1)");
    let pointer = |pos, button, pressed| egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![
        egui::Event::PointerMoved(position),
        pointer(position, egui::PointerButton::Secondary, true),
    ]);
    frame(vec![pointer(
        position,
        egui::PointerButton::Secondary,
        false,
    )]);
    let output = frame(vec![]);
    let delete = text_position(&output, "删除整个命名空间");
    frame(vec![
        egui::Event::PointerMoved(delete),
        pointer(delete, egui::PointerButton::Primary, true),
    ]);
    frame(vec![pointer(delete, egui::PointerButton::Primary, false)]);
    assert_eq!(requested, Some(b"app:".to_vec()));
}

#[test]
fn namespace_paths_keep_binary_literal_delimiters() {
    assert_eq!(
        namespace_prefix(&[b"app".to_vec(), b"users".to_vec()]),
        b"app:users:"
    );
    assert_eq!(
        namespace_prefix(&[b"a*?[x]\\\\".to_vec(), vec![0xff]]),
        b"a*?[x]\\\\:\xff:"
    );
    assert_eq!(namespace_prefix(&[Vec::new()]), b":");
}

#[test]
fn namespace_delete_requires_a_clean_stable_database() {
    let context = egui::Context::default();
    let mut doc = RedisDocument::fixture(&context);
    doc.value_text.push('x');
    assert!(!doc.preview_namespace(b"app:".to_vec()));
    assert!(!doc.busy);
    doc.value_text = doc.value_original.clone();
    doc.confirmation = Some(Confirmation::Namespace {
        token: 1,
        prefix: b"app:".to_vec(),
        count: 20,
        database: 0,
        notice: None,
    });
    assert!(!doc.select_database(1));
    assert!(!doc.disconnect());
    assert!(!doc.preview_namespace(b"cache:".to_vec()));
    assert!(!doc.busy);
}

#[test]
fn large_namespace_notice_keeps_confirmation_enabled() {
    for dark in [false, true] {
        let context = egui::Context::default();
        context.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut doc = RedisDocument::fixture(&context);
        let notice = "命名空间较大，删除可能耗时较长；请核对范围后确认执行。";
        doc.confirmation = Some(Confirmation::Namespace {
            token: 1,
            prefix: b"app:".to_vec(),
            count: 10_001,
            database: doc.database,
            notice: Some(notice.into()),
        });
        let mut time = 0.0;
        let mut frame = |events| {
            time += 0.1;
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 720.0),
                    )),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| doc.confirmation_ui(ui));
                },
            )
        };
        frame(vec![]);
        let output = frame(vec![]);
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::epaint::Shape::Text(text) if text.galley.text().contains(notice)
        )));
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "确认执行" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("confirmation button should remain visible");
        let pointer = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(position), pointer(true)]);
        frame(vec![pointer(false)]);
        assert!(doc.confirmation.is_none());
        assert!(doc.busy, "large namespace deletion must be submitted");
        assert!(doc.error.is_none());
    }
}

#[test]
fn namespaces_keep_binary_and_parent_keys_distinct() {
    let mut tree = KeyTree::default();
    for key in [b"a".as_slice(), b"a:b", b"a::b", b"a:\xff", b""] {
        tree.insert(key);
    }
    assert_eq!(tree.count, 5);
    assert_eq!(tree.children[b"a".as_slice()].key, Some(b"a".to_vec()));
    assert_eq!(tree.children[b"a".as_slice()].count, 4);
    assert!(
        tree.children[b"a".as_slice()]
            .children
            .contains_key(b"\xff".as_slice())
    );
    assert_eq!(tree.children[b"".as_slice()].key, Some(Vec::new()));
}

#[test]
fn bound_document_uses_saved_connection_without_connecting_or_configuration_ui() {
    let context = egui::Context::default();
    let profile = RedisProfile {
        name: "业务 Redis".into(),
        host: "cache.example.test".into(),
        database: 3,
        ..Default::default()
    };
    let mut doc = RedisDocument::new_bound(&context, profile.clone(), Credentials::default());
    assert_eq!(doc.profile_id(), profile.id);
    assert_eq!(doc.bound_profile().host, "cache.example.test");
    assert_eq!(doc.database_index(), 0);
    assert!(!doc.is_connected());
    assert!(!doc.busy);
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| doc.show(ui));
    });
    fn collect(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(shape) => text.push_str(shape.galley.text()),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut text);
    }
    assert!(text.contains("业务 Redis"));
    assert!(text.contains("连接 Redis"));
    assert!(!text.contains("保存连接"));
    assert!(!text.contains("ACL 用户名"));
    assert!(!doc.busy);
}

#[test]
fn connection_lifecycle_rejects_busy_dirty_and_pending_confirmation() {
    let context = egui::Context::default();
    let mut doc = RedisDocument::fixture(&context);
    doc.value_text.push_str("draft");
    assert!(!doc.connect());
    assert!(!doc.disconnect());
    assert!(!doc.select_database(1));
    assert_eq!(doc.database_index(), 0);
    doc.value_text = doc.value_original.clone();
    doc.confirmation = Some(Confirmation::Mutation {
        key: b"key".to_vec(),
        change: RedisMutation::Delete,
        label: "删除".into(),
    });
    assert!(!doc.disconnect());
    assert!(!doc.select_database(1));
    doc.confirmation = None;
    doc.busy = true;
    assert!(!doc.connect());
    assert!(!doc.disconnect());
}

#[test]
fn synchronous_validation_failure_keeps_string_draft_editable() {
    let context = egui::Context::default();
    let mut doc = RedisDocument::fixture(&context);
    doc.value_text = "x".repeat(sqlshop::redis_database::MAX_VALUE_BYTES + 1);
    doc.mutate(
        b"app:users:1001".to_vec(),
        RedisMutation::SetString {
            value: doc.value_text.as_bytes().to_vec(),
            expected: vec![],
        },
    );
    assert!(!doc.busy);
    assert!(!doc.stale);
    assert!(doc.is_dirty());
    assert!(doc.error.is_some());
}

#[test]
fn formatting_json_preserves_high_precision_numbers() {
    let json = r#"{"amount":0.123456789012345678901,"id":184467440737095516160}"#;
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let formatted = serde_json::to_string_pretty(&value).unwrap();
    assert!(formatted.contains("0.123456789012345678901"));
    assert!(formatted.contains("184467440737095516160"));
}

#[test]
fn editor_hex_roundtrip_and_json_validation() {
    let bytes = b"\x00\xff\"\n";
    assert_eq!(
        ValueFormat::Hex
            .bytes(&ValueFormat::Hex.show(bytes))
            .unwrap(),
        bytes
    );
    assert!(ValueFormat::Json.bytes("{invalid}").is_err());
    assert_eq!(ValueFormat::Text.bytes("").unwrap(), Vec::<u8>::new());
}

#[test]
fn disconnected_draft_blocks_reconnect_and_database_changes() {
    let context = egui::Context::default();
    let mut doc = RedisDocument::fixture(&context);
    doc.connected = false;
    doc.value_text.push_str("\nunsaved");
    assert!(doc.is_dirty());
    doc.request(RedisRequest::Connect {
        profile: doc.profile.clone(),
        credentials: Credentials::default(),
    });
    assert!(!doc.busy);
    assert!(doc.error.as_deref().unwrap().contains("草稿"));
    doc.request(RedisRequest::SelectDb(1));
    assert!(!doc.busy);
    assert_eq!(doc.database, 0);
    assert!(doc.value_text.ends_with("unsaved"));
}

#[test]
fn deeply_nested_keys_have_bounded_tree_depth() {
    let key = "a:".repeat(10_000).into_bytes();
    let mut tree = KeyTree::default();
    tree.insert(&key);
    let mut node = &tree;
    let mut depth = 0;
    while let Some(child) = node.children.values().next() {
        node = child;
        depth += 1;
    }
    assert_eq!(depth, 25);
    assert_eq!(node.key.as_deref(), Some(key.as_slice()));
}

#[test]
fn pending_confirmation_does_not_render_other_database_controls() {
    for dark in [false, true] {
        let context = egui::Context::default();
        context.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut doc = RedisDocument::fixture(&context);
        doc.confirmation = Some(Confirmation::Mutation {
            key: b"app:users:1001".to_vec(),
            change: RedisMutation::Delete,
            label: "删除目标键".into(),
        });
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 720.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| doc.show(ui));
            },
        );
        fn text(shape: &egui::epaint::Shape, labels: &mut String) {
            match shape {
                egui::epaint::Shape::Text(text) => labels.push_str(text.galley.text()),
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text(shape, labels);
                    }
                }
                _ => {}
            }
        }
        let mut labels = String::new();
        for shape in &output.shapes {
            text(&shape.shape, &mut labels);
        }
        assert!(labels.contains("确认写入"));
        assert!(!labels.contains("DB 索引由服务器验证"));
        assert!(!labels.contains("保存值（保留 TTL）"));
        assert!(doc.confirmation.is_some());
        assert!(!doc.busy);
    }
}

#[test]
fn member_changes_are_dirty_and_binary_remains_hex() {
    let mut member = MemberDraft::new(0, vec![], vec![0xff], 1.0);
    assert!(member.hex);
    assert!(!member.dirty());
    assert_eq!(member.bytes().unwrap(), vec![0xff]);
    member.score = 2.0;
    assert!(member.dirty());
}
