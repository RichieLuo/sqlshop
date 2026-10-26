//! Shell file dispatch and a per-user Windows GUI instance lease.
//!
//! The native mutex owns the profile-writing lifetime. JSON files are only a
//! bounded request inbox, never a lock or an instruction to execute content.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_PATHS: usize = 32;
const MAX_REQUEST_BYTES: usize = 128 * 1024;
const MAX_REQUEST_AGE: u64 = 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    created: u64,
    paths: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct Attachment {
    directory: Option<PathBuf>,
    pending: VecDeque<Vec<PathBuf>>,
    focus_pending: bool,
}

impl Attachment {
    pub fn attach_context(self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(context_id(), self));
        ctx.request_repaint();
    }
}

pub struct Instance {
    attachment: Attachment,
    #[cfg(target_os = "windows")]
    _lease: windows::Lease,
}

impl Instance {
    pub fn attachment(&self) -> Attachment {
        self.attachment.clone()
    }
}

fn context_id() -> egui::Id {
    egui::Id::new("shop-shell-open-v1")
}

fn queued(paths: Vec<PathBuf>) -> VecDeque<Vec<PathBuf>> {
    if paths.is_empty() {
        VecDeque::from([Vec::new()])
    } else {
        paths.into_iter().map(|path| vec![path]).collect()
    }
}

/// `--open` consumes its next argument literally, including a leading dash.
pub fn file_arguments(arguments: &[OsString]) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut arguments = arguments.iter();
    let mut literal = false;
    while let Some(argument) = arguments.next() {
        if !literal && argument == "--" {
            literal = true;
            continue;
        }
        let argument = if !literal && argument == "--open" {
            arguments.next().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "--open requires a file path")
            })?
        } else {
            if !literal && argument.to_string_lossy().starts_with('-') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unknown GUI option; use --open before a filename starting with '-'",
                ));
            }
            argument
        };
        if argument.is_empty() || paths.len() == MAX_PATHS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file-open request is empty or contains too many paths",
            ));
        }
        let path = PathBuf::from(argument);
        paths.push(if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        });
    }
    Ok(paths)
}

/// `None` means the request was delivered to the existing GUI; do not create eframe.
pub fn start(product: &str, arguments: &[OsString]) -> io::Result<Option<Instance>> {
    let paths = file_arguments(arguments)?;
    if product.is_empty()
        || product.len() > 32
        || !product
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid product name",
        ));
    }
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(|profile| PathBuf::from(profile).join("AppData/Local"))
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "user profile is unavailable")
            })?;
        start_in(product, &local, paths)
    }
    #[cfg(not(target_os = "windows"))]
    Ok(Some(Instance {
        attachment: Attachment {
            directory: None,
            pending: queued(paths),
            focus_pending: true,
        },
    }))
}

#[cfg(target_os = "windows")]
fn start_in(product: &str, local: &Path, paths: Vec<PathBuf>) -> io::Result<Option<Instance>> {
    let namespace = namespace(product, local);
    let (lease, primary) = windows::Lease::open(&namespace)?;
    let directory = local.join("ShopSuite/OpenInbox/v1").join(namespace);
    // A secondary may arrive before eframe or the first App constructor exists.
    // Both sides can create the inbox; neither clears pending requests on startup.
    fs::create_dir_all(&directory)?;
    if !primary {
        enqueue(&directory, paths)?;
        windows::allow_foreground();
        return Ok(None);
    }
    Ok(Some(Instance {
        attachment: Attachment {
            directory: Some(directory),
            pending: queued(paths),
            focus_pending: true,
        },
        _lease: lease,
    }))
}

#[cfg(target_os = "windows")]
fn namespace(product: &str, profile: &Path) -> String {
    // Stable across installations and immutable payload versions. Do not use
    // DefaultHasher, whose algorithm is not part of its persistence contract.
    let key = profile.as_os_str().to_string_lossy().to_lowercase();
    let hash = key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("ShopOpen{product}{hash:016x}")
}

fn now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(io::Error::other)
}

#[cfg(any(target_os = "windows", test))]
fn enqueue(directory: &Path, paths: Vec<PathBuf>) -> io::Result<()> {
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let request = Request {
        version: 1,
        created: now()?,
        paths,
    };
    validate(&request)?;
    let bytes = serde_json::to_vec(&request).map_err(io::Error::other)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file-open request is too large",
        ));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let name = format!(
        "request-{nonce:032x}-{:08x}-{:016x}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = directory.join(format!("{name}.tmp"));
    let complete = directory.join(format!("{name}.json"));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, complete)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn validate(request: &Request) -> io::Result<()> {
    let current = now()?;
    if request.version != 1
        || request.paths.len() > MAX_PATHS
        || current.saturating_sub(request.created) > MAX_REQUEST_AGE
        || request.created > current.saturating_add(300)
        || request
            .paths
            .iter()
            .any(|path| !path.is_absolute() || path.as_os_str().len() > 32767)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid file-open request",
        ));
    }
    Ok(())
}

fn read_next(directory: &Path) -> io::Result<Option<Vec<PathBuf>>> {
    let mut candidates = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("request-")
                && name.ends_with(".json")
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .take(128)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    candidates.sort();
    for path in candidates.into_iter().take(16) {
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let request = if bytes.len() <= MAX_REQUEST_BYTES {
            serde_json::from_slice::<Request>(&bytes)
                .ok()
                .filter(|request| validate(request).is_ok())
        } else {
            None
        };
        fs::remove_file(path)?;
        if let Some(request) = request {
            return Ok(Some(request.paths));
        }
    }
    Ok(None)
}

pub fn poll(ctx: &egui::Context) -> io::Result<Option<Vec<PathBuf>>> {
    poll_when(ctx, true)
}

/// Stage and focus incoming requests while a document confirmation is open,
/// but dequeue only when the caller can safely handle another document.
pub fn poll_when(ctx: &egui::Context, ready: bool) -> io::Result<Option<Vec<PathBuf>>> {
    let Some(mut inbox) = ctx.data(|data| data.get_temp::<Attachment>(context_id())) else {
        return Ok(None);
    };
    ctx.request_repaint_after(Duration::from_millis(200));
    if inbox.pending.is_empty()
        && let Some(directory) = &inbox.directory
        && let Some(paths) = read_next(directory)?
    {
        inbox.pending = queued(paths);
        inbox.focus_pending = true;
    }
    if inbox.focus_pending {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        inbox.focus_pending = false;
    }
    let paths = ready.then(|| inbox.pending.pop_front()).flatten();
    ctx.data_mut(|data| data.insert_temp(context_id(), inbox));
    Ok(paths)
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
            WAIT_TIMEOUT,
        },
        System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
    };

    pub(super) struct Lease {
        handle: HANDLE,
        owned: bool,
    }

    impl Lease {
        pub(super) fn open(namespace: &str) -> io::Result<(Self, bool)> {
            let name = format!("Local\\{namespace}")
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>();
            // The named mutex is protected by the current user's default DACL.
            let handle = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            let mut lease = Self {
                handle,
                owned: !existed,
            };
            if existed {
                match unsafe { WaitForSingleObject(handle, 0) } {
                    WAIT_TIMEOUT => return Ok((lease, false)),
                    WAIT_OBJECT_0 | WAIT_ABANDONED => lease.owned = true,
                    _ => return Err(io::Error::last_os_error()),
                }
            }
            Ok((lease, true))
        }
    }

    impl Drop for Lease {
        fn drop(&mut self) {
            // The lease stays on the main thread until run_native has returned.
            unsafe {
                if self.owned {
                    ReleaseMutex(self.handle);
                }
                CloseHandle(self.handle);
            }
        }
    }

    pub(super) fn allow_foreground() {
        // A shell-launched secondary can grant the existing GUI foreground rights.
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
    }
}

#[cfg(test)]
pub(crate) fn attach_paths_for_test(ctx: &egui::Context, paths: Vec<PathBuf>) {
    Attachment {
        directory: None,
        pending: queued(paths),
        focus_pending: true,
    }
    .attach_context(ctx);
}

#[cfg(test)]
#[path = "shell_open/tests.rs"]
mod tests;
