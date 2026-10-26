use super::*;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("shop-shell-test-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn arguments_preserve_spaces_chinese_and_explicit_leading_dashes() {
    let arguments = [
        "--open",
        "含空格 文件.sql",
        "--open",
        "-draft.sql",
        "--",
        "--sample.json",
    ]
    .map(OsString::from);
    let paths = file_arguments(&arguments).unwrap();
    assert_eq!(paths.len(), 3);
    assert_eq!(paths[0].file_name().unwrap(), "含空格 文件.sql");
    assert_eq!(paths[1].file_name().unwrap(), "-draft.sql");
    assert_eq!(paths[2].file_name().unwrap(), "--sample.json");
    assert!(paths.iter().all(|path| path.is_absolute()));
    assert!(file_arguments(&[OsString::from("--open")]).is_err());
    assert!(file_arguments(&[OsString::from("--unknown")]).is_err());
}

#[test]
fn atomic_inbox_ignores_partial_invalid_and_oversized_requests() {
    let directory = Directory::new();
    fs::write(directory.0.join("request-partial.tmp"), b"{").unwrap();
    fs::write(directory.0.join("request-invalid.json"), b"{}").unwrap();
    fs::write(
        directory.0.join("request-large.json"),
        vec![b' '; MAX_REQUEST_BYTES + 1],
    )
    .unwrap();
    let path = directory.0.join("中文 工作区.sql");
    enqueue(&directory.0, vec![path.clone()]).unwrap();
    assert_eq!(read_next(&directory.0).unwrap(), Some(vec![path]));
    assert!(read_next(&directory.0).unwrap().is_none());
    assert!(directory.0.join("request-partial.tmp").exists());
}

#[test]
fn deferred_document_keeps_its_request_and_emits_restore_and_focus() {
    let directory = Directory::new();
    let first = directory.0.join("first.rpshop");
    let second = directory.0.join("second.rpshop");
    let ctx = egui::Context::default();
    attach_paths_for_test(&ctx, vec![first.clone(), second.clone()]);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        assert_eq!(poll_when(ctx, false).unwrap(), None);
    });
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(commands.contains(&egui::ViewportCommand::Focus));
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
    assert_eq!(poll(&ctx).unwrap(), Some(vec![first]));
    assert_eq!(poll(&ctx).unwrap(), Some(vec![second]));
    assert!(poll(&ctx).unwrap().is_none());
}

#[cfg(target_os = "windows")]
#[test]
fn windows_lease_forwards_before_context_exists_and_releases_for_next_launch() {
    let directory = Directory::new();
    let role = "TestShop";
    let primary = start_in(role, &directory.0, Vec::new()).unwrap().unwrap();
    let path = directory.0.join("转发 空格.sql");
    let local = directory.0.clone();
    let forwarded = path.clone();
    std::thread::spawn(move || {
        assert!(start_in(role, &local, vec![forwarded]).unwrap().is_none());
    })
    .join()
    .unwrap();
    let ctx = egui::Context::default();
    primary.attachment().attach_context(&ctx);
    assert_eq!(poll(&ctx).unwrap(), Some(Vec::new()));
    assert_eq!(poll(&ctx).unwrap(), Some(vec![path]));
    let local = directory.0.clone();
    std::thread::spawn(move || {
        assert!(start_in(role, &local, Vec::new()).unwrap().is_none());
    })
    .join()
    .unwrap();
    assert_eq!(poll(&ctx).unwrap(), Some(Vec::new()));
    drop(primary);
    let replacement = start_in(role, &directory.0, Vec::new()).unwrap().unwrap();
    drop(replacement);
}
