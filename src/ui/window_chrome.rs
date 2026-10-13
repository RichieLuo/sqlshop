use eframe::egui::{self, Color32};

pub(super) const TITLE_HEIGHT: f32 = 44.0;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const CONTROL_WIDTH: f32 = 46.0;

pub(super) const fn leading_inset() -> f32 {
    if cfg!(target_os = "macos") { 78.0 } else { 0.0 }
}

pub(super) fn configure_window(cc: &eframe::CreationContext<'_>) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Graphics::Dwm::{
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
        };

        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(window) = handle.as_raw()
        {
            let preference = DWMWCP_ROUND;
            // The HWND belongs to this live viewport; DWM copies the preference value.
            let result = unsafe {
                DwmSetWindowAttribute(
                    window.hwnd.get() as _,
                    DWMWA_WINDOW_CORNER_PREFERENCE as _,
                    std::ptr::from_ref(&preference).cast(),
                    std::mem::size_of_val(&preference) as _,
                )
            };
            cc.egui_ctx.data_mut(|data| {
                data.insert_temp(egui::Id::new("native-window-rounded"), result >= 0);
            });
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cc;
}

pub(super) fn paint_border(ctx: &egui::Context) {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = ctx;
        return;
    }
    #[cfg(target_os = "windows")]
    {
        let dark = ctx.style().visuals.dark_mode;
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        let color = match (dark, focused) {
            (false, true) => Color32::from_rgb(110, 145, 143),
            (false, false) => Color32::from_rgb(155, 165, 170),
            (true, true) => Color32::from_rgb(100, 155, 150),
            (true, false) => Color32::from_rgb(95, 105, 115),
        };
        let rounded = ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::new("native-window-rounded"))
                .unwrap_or(false)
        });
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        let radius = if rounded && !maximized {
            (8.0 / ctx.pixels_per_point()).round() as u8
        } else {
            0
        };
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("window-border"),
        ))
        .rect_stroke(
            ctx.viewport_rect(),
            radius,
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Inside,
        );
    }
}

pub(super) fn controls(ui: &mut egui::Ui) {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let _ = ui;
        return;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (control, label, command) in [
                (WindowControl::Close, "关闭", egui::ViewportCommand::Close),
                (
                    WindowControl::Maximize { maximized },
                    if maximized { "还原" } else { "最大化" },
                    egui::ViewportCommand::Maximized(!maximized),
                ),
                (
                    WindowControl::Minimize,
                    "最小化",
                    egui::ViewportCommand::Minimized(true),
                ),
            ] {
                if control_button(ui, control).on_hover_text(label).clicked() {
                    ui.ctx().send_viewport_cmd(command);
                }
            }
        });
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[derive(Clone, Copy)]
enum WindowControl {
    Minimize,
    Maximize { maximized: bool },
    Close,
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn control_button(ui: &mut egui::Ui, control: WindowControl) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(CONTROL_WIDTH, TITLE_HEIGHT),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            match control {
                WindowControl::Minimize => "最小化",
                WindowControl::Maximize { maximized: true } => "还原",
                WindowControl::Maximize { maximized: false } => "最大化",
                WindowControl::Close => "关闭",
            },
        )
    });
    let highlighted = response.hovered() || response.is_pointer_button_down_on();
    let close = matches!(control, WindowControl::Close);
    let visuals = ui.style().interact(&response);
    if highlighted {
        let fill = if close {
            Color32::from_rgb(196, 43, 56)
        } else {
            visuals.weak_bg_fill
        };
        ui.painter().rect_filled(rect, 0, fill);
    }
    let focused = ui.input(|input| input.viewport().focused.unwrap_or(true));
    let color = if close && highlighted {
        Color32::WHITE
    } else if focused {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let stroke = egui::Stroke::new(1.2, color);
    let icon = egui::Rect::from_center_size(rect.center(), egui::vec2(10.0, 10.0));
    match control {
        WindowControl::Minimize => {
            ui.painter().line_segment(
                [
                    egui::pos2(icon.left(), rect.center().y),
                    egui::pos2(icon.right(), rect.center().y),
                ],
                stroke,
            );
        }
        WindowControl::Maximize { maximized: false } => {
            ui.painter()
                .rect_stroke(icon, 0, stroke, egui::StrokeKind::Inside);
        }
        WindowControl::Maximize { maximized: true } => {
            let front = icon.translate(egui::vec2(-1.0, 1.0)).shrink(1.0);
            let back = front.translate(egui::vec2(3.0, -3.0));
            ui.painter()
                .line_segment([back.left_top(), back.right_top()], stroke);
            ui.painter()
                .line_segment([back.right_top(), back.right_bottom()], stroke);
            ui.painter()
                .line_segment([back.left_top(), front.left_top()], stroke);
            ui.painter()
                .line_segment([back.right_bottom(), front.right_bottom()], stroke);
            ui.painter()
                .rect_stroke(front, 0, stroke, egui::StrokeKind::Inside);
        }
        WindowControl::Close => {
            ui.painter()
                .line_segment([icon.left_top(), icon.right_bottom()], stroke);
            ui.painter()
                .line_segment([icon.right_top(), icon.left_bottom()], stroke);
        }
    }
    if response.has_focus() {
        ui.painter()
            .rect_stroke(rect.shrink(2.0), 0, stroke, egui::StrokeKind::Inside);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn render(ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::top("title-test")
                    .exact_height(TITLE_HEIGHT)
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), controls);
                    });
            },
        )
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn caption_buttons_send_commands_from_the_top_right_edge() {
        for (offset, expected) in [
            (0.5, egui::ViewportCommand::Close),
            (1.5, egui::ViewportCommand::Maximized(true)),
            (2.5, egui::ViewportCommand::Minimized(true)),
        ] {
            let ctx = egui::Context::default();
            let _ = render(&ctx, vec![]);
            let pos = egui::pos2(900.0 - CONTROL_WIDTH * offset, 2.0);
            let _ = render(
                &ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            let output = render(
                &ctx,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(
                output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .contains(&expected)
            );
        }
    }
}

pub(super) fn drag_area(ui: &mut egui::Ui) {
    let response = ui.allocate_response(ui.available_size(), egui::Sense::click_and_drag());
    drag_response(&response);
}

pub(super) fn drag_response(response: &egui::Response) {
    let ctx = &response.ctx;
    if response.double_clicked() {
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if response.is_pointer_button_down_on()
        && ctx.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary))
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}
