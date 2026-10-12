// Opt-in main-thread test: observe the real Dock icon after eframe initializes.
#[cfg(target_os = "macos")]
#[path = "../src/branding.rs"]
mod branding;

#[cfg(target_os = "macos")]
fn main() {
    use eframe::egui;
    use objc2_app_kit::{NSApplication, NSBitmapImageRep};
    use objc2_foundation::MainThreadMarker;

    if std::env::var_os("SQLSHOP_TEST_NATIVE_ICON").is_none() {
        println!("Skipped: set SQLSHOP_TEST_NATIVE_ICON=1 to check the macOS Dock icon");
        return;
    }

    struct Probe {
        frames: usize,
        logo: egui::TextureHandle,
    }

    impl eframe::App for Probe {
        fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
            self.frames += 1;
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.image((self.logo.id(), egui::vec2(64.0, 64.0)));
                ui.label("SQLShop · Dock 图标启动检查");
            });
            if matches!(self.frames, 1 | 10 | 60) {
                let mtm = MainThreadMarker::new().unwrap();
                // SAFETY: AppKit runs on its main thread. Retained objects outlive
                // each read, and every queried pixel lies within the bitmap.
                unsafe {
                    let icon = NSApplication::sharedApplication(mtm)
                        .applicationIconImage()
                        .expect("eframe must set the Dock icon");
                    let tiff = icon.TIFFRepresentation().unwrap();
                    let bitmap = NSBitmapImageRep::initWithData(mtm.alloc(), &tiff).unwrap();
                    let width = bitmap.pixelsWide();
                    let height = bitmap.pixelsHigh();
                    assert!(width > 0 && height > 0);
                    for (x, y) in [
                        (0, 0),
                        (width - 1, 0),
                        (0, height - 1),
                        (width - 1, height - 1),
                    ] {
                        let alpha = bitmap.colorAtX_y(x, y).unwrap().alphaComponent();
                        assert!(
                            alpha < 0.01,
                            "Dock icon reverted to a square at frame {}",
                            self.frames
                        );
                    }
                    let alpha = bitmap
                        .colorAtX_y(width / 2, height / 8)
                        .unwrap()
                        .alphaComponent();
                    assert!(alpha > 0.99, "the backing plate must stay opaque");
                }
                println!(
                    "Dock icon has transparent corners and a solid plate at frame {}",
                    self.frames
                );
            }
            if self.frames >= 60 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SQLShop · Dock icon regression check")
            .with_icon(branding::icon_data())
            .with_inner_size([360.0, 150.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SQLShop icon check",
        options,
        Box::new(|cc| {
            Ok(Box::new(Probe {
                frames: 0,
                logo: branding::load_texture(&cc.egui_ctx),
            }))
        }),
    )
    .unwrap();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("Skipped: Dock icon checks require macOS");
}
