#[cfg(target_os = "windows")]
#[path = "src/branding_icon.rs"]
mod branding_icon;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/branding_icon.rs");
    println!("cargo:rerun-if-changed=assets/branding");
    #[cfg(target_os = "windows")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
}

#[cfg(target_os = "windows")]
fn embed_windows_icon() {
    use image::codecs::ico::{IcoEncoder, IcoFrame};

    let icon = branding_icon::icon_image();
    let frames: Vec<_> = [16, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|size| {
            let resized =
                image::imageops::resize(&icon, size, size, image::imageops::FilterType::Lanczos3);
            IcoFrame::as_png(
                resized.as_raw(),
                size,
                size,
                image::ExtendedColorType::Rgba8,
            )
            .expect("encode a Windows application icon frame")
        })
        .collect();
    let name = env!("CARGO_PKG_NAME");
    let path =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"))
            .join(format!("{name}.ico"));
    let file = std::fs::File::create(&path).expect("create the Windows application icon");
    IcoEncoder::new(file)
        .encode_images(&frames)
        .expect("write the Windows application icon");

    winres::WindowsResource::new()
        .set_icon(path.to_str().expect("UTF-8 application icon path"))
        .set("ProductName", "SQLShop")
        .set("FileDescription", env!("CARGO_PKG_DESCRIPTION"))
        .set("OriginalFilename", &format!("{name}.exe"))
        .compile()
        .expect("embed Windows application resources");
}
