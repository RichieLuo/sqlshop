#![cfg_attr(not(test), windows_subsystem = "windows")]

use std::{env, ffi::OsString, fs, io, path::Path, process::Command};

fn target_from(home: &Path) -> io::Result<std::path::PathBuf> {
    let pointer = fs::read_to_string(home.join("current.path"))?;
    let target = std::path::PathBuf::from(pointer.trim_end_matches(['\r', '\n']));
    if !target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The installed application target is missing or invalid. Run install.ps1 again.",
        ));
    }
    let target = target.canonicalize()?;
    if !target.is_file() || !target.starts_with(home.join("versions").canonicalize()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The installed application target is missing or invalid. Run install.ps1 again.",
        ));
    }
    Ok(target)
}

fn launch(home: &Path, arguments: impl Iterator<Item = OsString>) -> io::Result<()> {
    let target = target_from(home)?;
    Command::new(&target)
        .args(arguments)
        .current_dir(target.parent().expect("installed executable parent"))
        .spawn()?;
    Ok(())
}

#[cfg(windows)]
fn show_error(message: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            window: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let caption: Vec<u16> = "Shop launcher".encode_utf16().chain(Some(0)).collect();
    // The UTF-16 buffers remain alive for the synchronous message box call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10) };
}

#[cfg(not(windows))]
fn show_error(message: &str) {
    eprintln!("{message}");
}

fn main() {
    let result = env::current_exe().and_then(|executable| {
        let home = executable
            .parent()
            .ok_or_else(|| io::Error::other("Missing launcher directory"))?;
        launch(home, env::args_os().skip(1))
    });
    if let Err(error) = result {
        show_error(&format!(
            "Could not start the latest Shop application.\n\n{error}"
        ));
        std::process::exit(1);
    }
}
