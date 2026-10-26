const CANVAS_SIZE: u32 = 1024;
const PLATE_SIZE: u32 = 820;
const ARTWORK_SIZE: u32 = 736;
const MIN_ALPHA: u8 = 8;

pub fn icon_image() -> image::RgbaImage {
    let source =
        image::load_from_memory(include_bytes!("../assets/branding/sqlshop-lynx-icon.png"))
            .expect("the embedded SQLShop application artwork must be a valid PNG")
            .into_rgba8();
    let (left, top, width, height) = visible_bounds(&source);
    let artwork = image::imageops::crop_imm(&source, left, top, width, height).to_image();
    // Existing tiles fill the plate; a transparent mascot has its own inset.
    // Both paths share the same final silhouette, independent of source padding.
    let limit = if false { PLATE_SIZE } else { ARTWORK_SIZE };
    let scale = limit as f64 / f64::from(width.max(height));
    let scaled = image::imageops::resize(
        &artwork,
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let mut icon =
        image::RgbaImage::from_pixel(CANVAS_SIZE, CANVAS_SIZE, image::Rgba([240, 246, 242, 255]));
    image::imageops::overlay(
        &mut icon,
        &scaled,
        i64::from((CANVAS_SIZE - scaled.width()) / 2),
        i64::from((CANVAS_SIZE - scaled.height()) / 2),
    );
    // Match SQLShop's native icon: an 80% rounded plate and 10% outer padding.
    let center = CANVAS_SIZE as f32 / 2.0;
    let half_size = CANVAS_SIZE as f32 * 0.4;
    let radius = half_size * 0.44;
    for (x, y, pixel) in icon.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5 - center).abs() - (half_size - radius);
        let dy = (y as f32 + 0.5 - center).abs() - (half_size - radius);
        let distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
        pixel[3] = ((0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
        if pixel[3] == 0 {
            pixel.0 = [0; 4];
        }
    }
    icon
}

fn visible_bounds(source: &image::RgbaImage) -> (u32, u32, u32, u32) {
    let (mut left, mut top) = source.dimensions();
    let (mut right, mut bottom) = (0, 0);
    for (x, y, pixel) in source.enumerate_pixels() {
        // Generated artwork contains near-transparent stray pixels outside the tile.
        if pixel[3] >= MIN_ALPHA {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    assert!(
        right > left && bottom > top,
        "application artwork must contain visible pixels"
    );
    (left, top, right - left, bottom - top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_icon_uses_the_shared_plate_bounds_at_every_windows_size() {
        let icon = icon_image();
        assert_eq!(icon.dimensions(), (1024, 1024));
        for size in [16, 24, 32, 48, 64, 128, 256] {
            let resized =
                image::imageops::resize(&icon, size, size, image::imageops::FilterType::Lanczos3);
            let mut left = size;
            let mut top = size;
            let mut right = 0;
            let mut bottom = 0;
            for (x, y, pixel) in resized.enumerate_pixels() {
                if pixel[3] >= 128 {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x + 1);
                    bottom = bottom.max(y + 1);
                }
            }
            let expected_margin = (size as f32 * 0.1).round() as u32;
            assert_eq!(
                (left, top, right, bottom),
                (
                    expected_margin,
                    expected_margin,
                    size - expected_margin,
                    size - expected_margin
                ),
                "icon silhouette differs at {size}px"
            );
            assert_eq!(resized.get_pixel(size / 2, size / 2)[3], 255);
            assert_eq!(resized.get_pixel(0, 0)[3], 0);
        }
        if let Some(path) = std::env::var_os("SHOP_ICON_PREVIEW") {
            icon.save(path).expect("save the application icon preview");
        }
    }

    #[test]
    fn source_bounds_ignore_near_transparent_noise() {
        let mut source = image::RgbaImage::new(20, 24);
        source.put_pixel(0, 0, image::Rgba([255, 255, 255, 1]));
        source.put_pixel(19, 23, image::Rgba([255, 255, 255, 7]));
        source.put_pixel(4, 5, image::Rgba([0, 100, 200, 255]));
        source.put_pixel(15, 18, image::Rgba([0, 100, 200, 255]));
        assert_eq!(visible_bounds(&source), (4, 5, 12, 14));
    }
}
