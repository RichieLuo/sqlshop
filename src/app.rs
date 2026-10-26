use crate::{branding, shell_open, ui};

pub fn run() -> eframe::Result {
    let screenshot_mode = std::env::var_os("SQLSHOP_SCREENSHOT").is_some();
    let instance = if screenshot_mode {
        None
    } else {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        let Some(instance) = shell_open::start("SQLShop", &arguments)
            .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?
        else {
            return Ok(());
        };
        Some(instance)
    };
    let attachment = instance.as_ref().map(shell_open::Instance::attachment);
    #[cfg(target_os = "macos")]
    branding::enable_miniwindow_icons();

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("SQLShop")
        .with_decorations(!cfg!(target_os = "windows"))
        .with_icon(branding::icon_data())
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([900.0, 600.0]);
    #[cfg(target_os = "macos")]
    {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_titlebar_buttons_shown(true);
    }
    if screenshot_mode
        && let Ok(size) = std::env::var("SQLSHOP_SCREENSHOT_SIZE")
        && let Some((width, height)) = size.split_once('x')
        && let (Ok(width), Ok(height)) = (width.parse::<u16>(), height.parse::<u16>())
        && (900..=4096).contains(&width)
        && (600..=2160).contains(&height)
    {
        viewport = viewport.with_inner_size([f32::from(width), f32::from(height)]);
    }
    // Keep screenshot runs independent of the user's saved window and panel layout.
    let screenshot_storage = if screenshot_mode {
        Some(tempfile::tempdir().map_err(|error| eframe::Error::AppCreation(Box::new(error)))?)
    } else {
        None
    };
    let options = eframe::NativeOptions {
        viewport,
        persist_window: !screenshot_mode,
        persistence_path: screenshot_storage
            .as_ref()
            .map(|directory| directory.path().join("ui.ron")),
        ..Default::default()
    };
    let result = eframe::run_native(
        "SQLShop",
        options,
        Box::new(move |cc| {
            if let Some(attachment) = attachment {
                attachment.attach_context(&cc.egui_ctx);
            }
            Ok(Box::new(ui::App::new(cc)))
        }),
    );
    drop(instance);
    result
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    #[derive(Default)]
    struct MemoryStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }

        fn flush(&mut self) {}
    }

    fn test_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn sidebar_width_survives_memory_round_trip() {
        let context = egui::Context::default();
        let _ = context.run(test_input(), |ctx| {
            egui::SidePanel::left("sidebar")
                .exact_width(310.0)
                .show(ctx, |_| {});
        });
        let mut storage = MemoryStorage::default();
        context.memory(|memory| eframe::set_value(&mut storage, "egui", memory));

        let restored = egui::Context::default();
        restored.memory_mut(|memory| *memory = eframe::get_value(&storage, "egui").unwrap());
        let _ = restored.run(test_input(), |ctx| {
            let panel = egui::SidePanel::left("sidebar")
                .default_width(240.0)
                .width_range(200.0..=340.0)
                .show(ctx, |ui| ui.set_min_width(ui.available_width()));
            assert_eq!(panel.response.rect.width(), 310.0);
        });
    }
}
