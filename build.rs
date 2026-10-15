#[cfg(target_os = "windows")]
#[path = "src/branding_icon.rs"]
mod branding_icon;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(target_os = "windows")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
}

#[cfg(target_os = "windows")]
fn embed_windows_icon() {
    use image::codecs::ico::{IcoEncoder, IcoFrame};

    println!("cargo:rerun-if-changed=src/branding_icon.rs");
    println!("cargo:rerun-if-changed=assets/branding/sqlshop-lynx-icon.png");

    let icon = branding_icon::rounded_icon_image();
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
            .expect("encode the Windows application icon")
        })
        .collect();
    let path =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"))
            .join("sqlshop.ico");
    let file = std::fs::File::create(&path).expect("create the Windows application icon");
    IcoEncoder::new(file)
        .encode_images(&frames)
        .expect("write the Windows application icon");

    winres::WindowsResource::new()
        .set_icon(path.to_str().expect("UTF-8 icon path"))
        .compile()
        .expect("embed the Windows application icon in the executable");
}
