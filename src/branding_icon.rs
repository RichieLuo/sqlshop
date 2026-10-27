const CANVAS_SIZE: u32 = 1024;
// Windows already sizes icons to their display slot; extra padding makes them small.
const OUTER_MARGIN: f32 = if cfg!(target_os = "windows") {
    0.0
} else {
    0.1
};
const ARTWORK_SIZE: u32 = if cfg!(target_os = "windows") {
    920
} else {
    736
};
const MIN_ALPHA: u8 = 8;

pub fn icon_image() -> image::RgbaImage {
    let source =
        image::load_from_memory(include_bytes!("../assets/branding/sqlshop-lynx-icon.png"))
            .expect("the embedded SQLShop application artwork must be a valid PNG")
            .into_rgba8();
    let (left, top, width, height) = visible_bounds(&source);
    let artwork = image::imageops::crop_imm(&source, left, top, width, height).to_image();
    // Preserve the mascot's inset relative to the plate as the whole icon grows.
    let scale = ARTWORK_SIZE as f64 / f64::from(width.max(height));
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
    // Keep the shared rounded silhouette, with no extra inset on Windows.
    let center = CANVAS_SIZE as f32 / 2.0;
    let half_size = CANVAS_SIZE as f32 * (0.5 - OUTER_MARGIN);
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
            let expected_margin = if cfg!(target_os = "windows") {
                0
            } else {
                (size as f32 * 0.1).round() as u32
            };
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
            // Lanczos can leave faint ringing at rounded corners in small frames.
            for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
                assert!(resized.get_pixel(x, y)[3] < MIN_ALPHA);
            }
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
