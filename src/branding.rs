use eframe::egui::{self, IconData};

#[cfg(target_os = "macos")]
pub fn enable_miniwindow_icons() {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSString, NSUserDefaults};

    let defaults = unsafe { NSUserDefaults::standardUserDefaults() };
    let key = NSString::from_str("AppleDockIconEnabled");
    let value = Retained::into_super(Retained::into_super(NSNumber::numberWithBool(true)));
    let registration = NSDictionary::<NSString, NSObject>::from_id_slice(&[&*key], &[value]);
    let registration: Retained<NSDictionary<NSString, AnyObject>> =
        unsafe { Retained::cast(registration) };
    unsafe { defaults.registerDefaults(&registration) };
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const APP_ICON_PNG: &[u8] = include_bytes!("../assets/branding/sqlshop-lynx-app-icon.png");
const LOGO_PNG: &[u8] = include_bytes!("../assets/branding/sqlshop-lynx-icon.png");

pub fn icon_data() -> IconData {
    #[cfg(target_os = "macos")]
    {
        // eframe installs this icon after application creation, so the viewport
        // must carry the rounded image rather than a separate startup override.
        image_data(icon_image())
    }
    #[cfg(target_os = "windows")]
    {
        image_data(image::imageops::resize(
            &icon_image(),
            256,
            256,
            image::imageops::FilterType::Lanczos3,
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        decode_icon(APP_ICON_PNG)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[path = "branding_icon.rs"]
mod branding_icon;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use branding_icon::icon_image;

fn decode_icon(png: &[u8]) -> IconData {
    let image = image::load_from_memory(png)
        .expect("the embedded SQLShop brand image must be a valid PNG")
        .resize(256, 256, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    image_data(image)
}

fn image_data(image: image::RgbaImage) -> IconData {
    IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}

pub fn load_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let icon = decode_icon(LOGO_PNG);
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    ctx.load_texture("sqlshop-lynx", image, egui::TextureOptions::LINEAR)
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_icon_has_a_solid_plate_and_transparent_rounded_exterior() {
        let icon = super::icon_data();
        assert_eq!((icon.width, icon.height), (256, 256));
        let image = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba).unwrap();
        for (x, y) in [(0, 0), (255, 0), (0, 255), (255, 255), (28, 28)] {
            assert_eq!(image.get_pixel(x, y)[3], 0);
        }
        for (x, y) in [(128, 32), (32, 128), (128, 128), (128, 223)] {
            assert_eq!(image.get_pixel(x, y)[3], 255);
        }
        assert!(image.rows().next().unwrap().all(|pixel| pixel[3] == 0));
        assert!(image.rows().next_back().unwrap().all(|pixel| pixel[3] == 0));
    }

    #[test]
    fn rounded_icon_has_a_solid_plate_and_transparent_rounded_exterior() {
        let image = super::icon_image();
        assert_eq!(image.dimensions(), (1024, 1024));
        for (x, y) in [(0, 0), (1023, 0), (0, 1023), (1023, 1023), (110, 110)] {
            assert_eq!(image.get_pixel(x, y)[3], 0);
        }
        // Both the pale backing plate and the animal artwork must be opaque.
        for (x, y) in [(512, 125), (125, 512), (512, 512), (512, 898)] {
            assert_eq!(image.get_pixel(x, y)[3], 255);
        }
        assert!(image.rows().next().unwrap().all(|pixel| pixel[3] == 0));
        assert!(image.rows().next_back().unwrap().all(|pixel| pixel[3] == 0));
        if let Some(path) = std::env::var_os("SQLSHOP_ICON_PREVIEW") {
            image.save(path).unwrap();
        }
    }
}
