#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod branding;
mod ui;

fn main() -> eframe::Result {
    app::run()
}
