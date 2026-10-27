use super::*;

#[test]
fn table_footer_controls_stay_on_one_line_at_all_workspace_widths() {
    for width in [406.0, 554.0, 934.0] {
        let (mut app, _, change) = table_flow_fixture();
        app.tabs[0].stage_cell(change);
        app.tabs[0].browse.offset = 200;
        app.tabs[0].has_more = true;
        let ctx = app.context.clone();
        install_font(&ctx);
        let available = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(width, 450.0));
        let (content, footer) = table_workspace_rects(available);
        assert_eq!(footer.height(), 42.0);
        assert!(content.bottom() < footer.top());
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                        ui.set_clip_rect(ui.clip_rect().intersect(footer));
                        app.table_pagination(ui);
                    });
                });
            },
        );
        // Narrow footers clip later controls until horizontally scrolled.
        let expected = if width < 600.0 {
            vec!["+", "−", "刷新", "上一页"]
        } else {
            vec!["+", "−", "刷新", "上一页", "下一页", "每页", "200"]
        };
        for label in expected {
            let shape = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => Some(text),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {label} at width {width}"));
            let rect = egui::Rect::from_min_size(shape.pos, shape.galley.size());
            assert!(
                rect.top() >= footer.top() && rect.bottom() <= footer.bottom(),
                "{label} at {rect:?} outside footer {footer:?}, width {width}"
            );
        }
    }
}

#[test]
fn table_footer_hides_draft_actions_and_record_navigation_without_changes() {
    let (mut app, _, _) = table_flow_fixture();
    let ctx = app.context.clone();
    install_font(&ctx);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.table_pagination(ui));
    });
    let labels = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for label in ["+", "−", "刷新", "首页", "上一页", "下一页", "末页"] {
        assert!(labels.contains(&label), "missing {label}");
    }
    for label in ["提交全部", "撤销暂存", "|‹", "‹", "›", "›|"] {
        assert!(!labels.contains(&label), "unexpected {label}");
    }
    assert!(!labels.iter().any(|label| label.contains("条 /")));
}

#[test]
fn theme_text_colors_remain_readable_in_light_and_dark_modes() {
    let ctx = egui::Context::default();
    for (visuals, text, secondary) in [
        (
            egui::Visuals::light(),
            Color32::from_rgb(30, 40, 48),
            Color32::from_rgb(71, 84, 94),
        ),
        (
            egui::Visuals::dark(),
            Color32::from_rgb(230, 236, 240),
            Color32::from_rgb(170, 184, 194),
        ),
    ] {
        ctx.set_visuals(visuals);
        apply_theme(&ctx);
        let style = ctx.style();
        assert_eq!(style.visuals.text_color(), text);
        assert_eq!(style.visuals.weak_text_color(), secondary);
        assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, text);
        assert_eq!(style.visuals.widgets.hovered.fg_stroke.color, text);
    }
}

#[test]
fn short_dialog_footer_stays_next_to_content_without_filling_window_height() {
    for dark in [false, true] {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        apply_theme(&ctx);
        install_font(&ctx);
        for frame in 0..3 {
            let _ =
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1280.0, 820.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        let mut footer = egui::Rect::NOTHING;
                        let dialog = dialog_window("退出前确认", ctx).show(ctx, |ui| {
                    ui.label("退出将丢弃表数据暂存，未提交事务可能回滚；SQL 草稿可选择保存。");
                    footer = dialog_actions(ui, |ui| {
                        ui.add(primary_button(ui, "保存草稿并退出"));
                        ui.add(danger_button(ui, "不保存草稿，直接退出"));
                        ui.button("取消").clicked()
                    }).response.rect;
                }).unwrap().response.rect;
                        // egui measures a new window before applying its center
                        // pivot. Compare child and window coordinates only once
                        // the window position has settled.
                        if frame < 2 {
                            return;
                        }
                        assert!(footer.height() <= 40.0, "footer expanded: {footer:?}");
                        // Allow the shared 24 px padding and 20 px title while
                        // still rejecting a footer that consumes viewport height.
                        assert!(dialog.height() < 240.0, "short dialog expanded: {dialog:?}");
                        assert!(
                            // 24 px frame padding plus the 8 px item gap and stroke.
                            dialog.bottom() - footer.bottom() <= 34.0,
                            "blank space below footer: dialog={dialog:?}, footer={footer:?}"
                        );
                        assert!(dialog.contains_rect(footer));
                    },
                );
        }
    }
}

#[test]
fn dialog_windows_open_centered_on_screen() {
    let ctx = egui::Context::default();
    let mut rect = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        let response = dialog_window("居中测试", ctx)
            .default_width(400.0)
            .show(ctx, |ui| ui.label("内容"));
        rect = response.map(|inner| inner.response.rect);
    });
    let rect = rect.expect("dialog shown");
    assert!(rect.width() > 0.0 && rect.height() > 0.0);
    let content = ctx.content_rect();
    assert!(
        (rect.center().x - content.center().x).abs() <= 1.0,
        "dialog center x {} vs content center {}",
        rect.center().x,
        content.center().x
    );
    assert!(
        (rect.center().y - content.center().y).abs() <= 1.0,
        "dialog center y {} vs content center {}",
        rect.center().y,
        content.center().y
    );
}

#[test]
fn table_toolbar_is_one_row_and_filters_start_collapsed() {
    let mut app = session_test_app();
    assert!(!app.tabs[0].filters_open);
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let toolbar = ui.scope(|ui| app.table_controls(ui));
            assert!(toolbar.response.rect.height() <= 36.0);
            app.tabs[0].filters_open = true;
            let expanded = ui.scope(|ui| app.table_controls(ui));
            assert!(expanded.response.rect.height() > toolbar.response.rect.height());
        });
    });
}

#[test]
fn table_footer_stays_below_content_and_inside_window() {
    for size in [egui::vec2(1280.0, 820.0), egui::vec2(900.0, 600.0)] {
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let available = ui.available_rect_before_wrap();
                    let (content, footer) = table_workspace_rects(available);
                    assert!(content.bottom() < footer.top());
                    assert!(available.contains_rect(footer));
                    assert_eq!(footer.height(), 42.0);
                    // A child with oversized content cannot move the reserved footer.
                    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                        ui.set_clip_rect(ui.clip_rect().intersect(content));
                        ui.allocate_space(egui::vec2(content.width(), 2000.0));
                    });
                    ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                        assert_eq!(ui.max_rect(), footer);
                        assert!(ui.max_rect().bottom() <= size.y);
                    });
                });
            },
        );
    }
}

#[test]
fn dragging_splitter_resizes_editor_and_reserves_result_space() {
    let ctx = egui::Context::default();
    let mut tab = QueryTab::new(1);
    let mut height = Some(180.0);
    let mut frame = |events| {
        let mut splitter = egui::Rect::NOTHING;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    splitter = sql_editor(ui, &mut tab, &mut height, true, &[], &[], Engine::MySql)
                        .0
                        .rect;
                });
            },
        );
        (splitter, height)
    };
    let (rect, _) = frame(Vec::new());
    let start = rect.center();
    frame(vec![
        egui::Event::PointerMoved(start),
        egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    let (_, grown) = frame(vec![egui::Event::PointerMoved(
        start + egui::vec2(0.0, 60.0),
    )]);
    assert!(grown.unwrap() > 180.0);
    let (_, clamped) = frame(vec![egui::Event::PointerMoved(
        start + egui::vec2(0.0, 1000.0),
    )]);
    assert!(clamped.unwrap() <= 420.0);
    let (_, shrunk) = frame(vec![egui::Event::PointerMoved(
        start - egui::vec2(0.0, 1000.0),
    )]);
    assert_eq!(shrunk, Some(100.0));
    frame(vec![egui::Event::PointerButton {
        pos: start - egui::vec2(0.0, 1000.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    let (rect, _) = frame(Vec::new());
    let reset_position = rect.center();
    for _ in 0..2 {
        frame(vec![
            egui::Event::PointerMoved(reset_position),
            egui::Event::PointerButton {
                pos: reset_position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        frame(vec![egui::Event::PointerButton {
            pos: reset_position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    assert_eq!(frame(Vec::new()).1, None);
}
