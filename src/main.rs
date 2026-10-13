#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod branding;
mod ui;

fn main() -> eframe::Result {
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
    if std::env::var_os("SQLSHOP_SCREENSHOT").is_some()
        && let Ok(size) = std::env::var("SQLSHOP_SCREENSHOT_SIZE")
        && let Some((width, height)) = size.split_once('x')
        && let (Ok(width), Ok(height)) = (width.parse::<u16>(), height.parse::<u16>())
        && (900..=4096).contains(&width)
        && (600..=2160).contains(&height)
    {
        viewport = viewport.with_inner_size([f32::from(width), f32::from(height)]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "SQLShop",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
    )
}
