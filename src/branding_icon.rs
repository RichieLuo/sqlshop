pub fn rounded_icon_image() -> image::RgbaImage {
    let logo = image::load_from_memory(include_bytes!("../assets/branding/sqlshop-lynx-icon.png"))
        .expect("the embedded SQLShop logo must be a valid PNG")
        .resize_exact(760, 760, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let mut image = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([240, 246, 242, 255]));
    image::imageops::overlay(&mut image, &logo, 132, 132);
    // Render a consistent native silhouette, including transparent outer padding.
    // The backing plate stays opaque; a drawn rounded border alone is insufficient.
    let center = 512.0_f32;
    let half_size = 409.6_f32;
    let radius = 180.224_f32;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5 - center).abs() - (half_size - radius);
        let dy = (y as f32 + 0.5 - center).abs() - (half_size - radius);
        let distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
        pixel[3] = ((0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
        if pixel[3] == 0 {
            pixel.0 = [0; 4];
        }
    }
    image
}
