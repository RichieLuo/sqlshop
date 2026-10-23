use eframe::egui::{self, Color32};
use sqlshop::model::Engine;

pub(super) fn paint_redis_connection(ui: &egui::Ui, rect: egui::Rect, connected: bool) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let scale = rect.width() / 24.0;
    let point = |x: f32, y: f32| rect.min + egui::vec2(x, y) * scale;
    let painter = ui.painter();
    let red = if ui.visuals().dark_mode {
        Color32::from_rgb(239, 92, 83)
    } else {
        Color32::from_rgb(190, 42, 36)
    };
    let edge = if ui.visuals().dark_mode {
        Color32::from_rgb(165, 47, 44)
    } else {
        Color32::from_rgb(143, 30, 27)
    };
    // Three stacked red lozenges distinguish Redis without changing its connection name.
    for y in [14.0, 9.0, 4.0] {
        painter.add(egui::Shape::convex_polygon(
            vec![
                point(2.0, y + 3.0),
                point(12.0, y - 0.5),
                point(22.0, y + 3.0),
                point(22.0, y + 5.0),
                point(12.0, y + 8.5),
                point(2.0, y + 5.0),
            ],
            edge,
            egui::Stroke::NONE,
        ));
        painter.add(egui::Shape::convex_polygon(
            vec![
                point(2.0, y + 3.0),
                point(12.0, y - 0.5),
                point(22.0, y + 3.0),
                point(12.0, y + 6.5),
            ],
            red,
            egui::Stroke::NONE,
        ));
    }
    let mark = Color32::from_rgb(255, 237, 225);
    painter.line_segment(
        [point(7.0, 7.0), point(11.0, 8.0)],
        egui::Stroke::new(1.5 * scale, mark),
    );
    painter.circle_filled(point(14.0, 6.5), 1.0 * scale, mark);
    painter.add(egui::Shape::convex_polygon(
        vec![
            point(14.0, 8.5),
            point(17.0, 7.5),
            point(19.0, 8.5),
            point(16.0, 9.5),
        ],
        mark,
        egui::Stroke::NONE,
    ));
    let center = rect.right_bottom() - egui::vec2(0.5, 0.5);
    painter.circle_filled(center, 3.5, super::surface(ui));
    if connected {
        painter.circle_filled(center, 2.5, super::accent(ui));
    } else {
        painter.circle_stroke(
            center,
            2.0,
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
    }
}

pub(super) fn paint_connection(ui: &egui::Ui, rect: egui::Rect, engine: Engine, connected: bool) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let dark = ui.visuals().dark_mode;
    let color = match engine {
        Engine::MySql => {
            if dark {
                Color32::from_rgb(102, 190, 232)
            } else {
                Color32::from_rgb(0, 104, 150)
            }
        }
        Engine::Postgres => {
            if dark {
                Color32::from_rgb(151, 181, 240)
            } else {
                Color32::from_rgb(51, 103, 145)
            }
        }
        Engine::SqlServer => {
            if dark {
                Color32::from_rgb(255, 143, 143)
            } else {
                Color32::from_rgb(181, 52, 52)
            }
        }
    };
    let painter = ui.painter();
    let scale = rect.width() / 24.0;
    let point = |x: f32, y: f32| rect.min + egui::vec2(x, y) * scale;
    let stroke = egui::Stroke::new(1.15, color);
    let line = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| point(x, y)).collect(),
            stroke,
        ));
    };
    match engine {
        Engine::MySql => {
            // A leaping dolphin: dorsal fin, long snout, flipper and forked tail.
            let outline = [
                (2.0, 16.0),
                (4.0, 11.0),
                (7.0, 7.5),
                (9.0, 6.5),
                (9.5, 2.5),
                (12.0, 5.8),
                (15.0, 5.5),
                (17.8, 6.5),
                (19.0, 8.5),
                (22.0, 10.0),
                (22.0, 11.0),
                (18.0, 10.5),
                (15.0, 12.0),
                (12.0, 13.0),
                (10.5, 17.0),
                (9.0, 17.5),
                (9.5, 13.5),
                (6.0, 14.0),
                (4.0, 16.5),
                (5.5, 20.0),
                (2.5, 18.5),
                (0.8, 19.0),
                (2.0, 16.0),
            ];
            painter.add(egui::Shape::closed_line(
                outline.iter().map(|&(x, y)| point(x, y)).collect(),
                stroke,
            ));
            line(&[(4.0, 13.5), (7.0, 10.5), (11.0, 8.5), (15.0, 8.0)]);
            painter.circle_filled(point(17.0, 8.0), 0.8 * scale, color);
        }
        Engine::Postgres => {
            // Broad ears frame the head; the curled trunk makes the elephant readable.
            let outline = [
                (8.0, 3.5),
                (5.0, 2.5),
                (2.0, 4.5),
                (1.5, 8.5),
                (2.5, 13.5),
                (5.0, 17.0),
                (7.5, 16.0),
                (9.0, 18.0),
                (10.0, 17.0),
                (10.5, 20.0),
                (12.0, 22.0),
                (15.0, 22.0),
                (17.0, 20.0),
                (16.5, 18.5),
                (14.5, 20.0),
                (13.0, 19.5),
                (13.0, 15.0),
                (15.5, 16.5),
                (18.5, 16.0),
                (21.5, 12.0),
                (22.0, 7.5),
                (21.0, 4.0),
                (18.0, 2.5),
                (15.5, 3.5),
                (12.0, 2.0),
                (8.0, 3.5),
            ];
            painter.add(egui::Shape::closed_line(
                outline.iter().map(|&(x, y)| point(x, y)).collect(),
                stroke,
            ));
            line(&[(8.0, 4.0), (6.0, 7.0), (6.5, 12.0), (8.0, 15.5)]);
            line(&[(15.5, 4.0), (17.0, 7.0), (16.5, 12.0), (14.5, 14.5)]);
            line(&[(10.0, 11.0), (10.5, 15.0), (10.5, 18.0)]);
            line(&[(13.5, 10.5), (13.0, 15.0)]);
            painter.circle_filled(point(8.5, 8.5), 0.8 * scale, color);
            painter.circle_filled(point(14.5, 8.5), 0.8 * scale, color);
            line(&[(8.0, 12.5), (9.0, 15.0), (10.0, 14.0)]);
        }
        Engine::SqlServer => {
            // A twisted red ribbon, built as a mesh rather than a filled badge.
            let left = [
                (3.0, 2.0),
                (6.5, 6.0),
                (9.5, 10.0),
                (8.0, 14.0),
                (3.0, 21.0),
            ];
            let right = [
                (20.0, 4.0),
                (17.5, 8.0),
                (15.0, 12.0),
                (17.0, 16.0),
                (21.0, 22.0),
            ];
            line(&left);
            line(&right);
            for row in 0..left.len() {
                line(&[left[row], right[row]]);
                if row + 1 < left.len() {
                    line(&[left[row], right[row + 1]]);
                    line(&[right[row], left[row + 1]]);
                }
            }
            for fraction in [0.33, 0.66] {
                let points: Vec<_> = left
                    .iter()
                    .zip(right.iter())
                    .map(|(&(lx, ly), &(rx, ry))| {
                        (lx + (rx - lx) * fraction, ly + (ry - ly) * fraction)
                    })
                    .collect();
                line(&points);
            }
        }
    }
    let center = rect.right_bottom() - egui::vec2(0.5, 0.5);
    painter.circle_filled(center, 3.5, super::surface(ui));
    if connected {
        painter.circle_filled(center, 2.5, super::accent(ui));
    } else {
        painter.circle_stroke(
            center,
            2.0,
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
    }
}
