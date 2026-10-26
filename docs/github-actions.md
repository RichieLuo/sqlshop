# GitHub Actions 构建与下载

三个 Shop 仓库使用相同的 `.github/workflows/release.yml`，在各系统的 GitHub 托管运行器上编译并打包，不需要在本机安装三套操作系统。

## 构建平台与产物

| 平台 | 运行器 | Rust 目标 | 程序包 |
| --- | --- | --- | --- |
| Windows x64 | `windows-2022` | `x86_64-pc-windows-msvc` | `<Product>-<version>-windows-x64.zip` |
| macOS Intel | `macos-15-intel` | `x86_64-apple-darwin` | `<Product>-<version>-macos-x64.tar.gz` |
| macOS Apple Silicon | `macos-15` | `aarch64-apple-darwin` | `<Product>-<version>-macos-arm64.tar.gz` |
| Linux x64 | `ubuntu-24.04` | `x86_64-unknown-linux-gnu` | `<Product>-<version>-linux-x64.tar.gz` |

`<Product>` 为 SQLShop、LinkShop 或 RPShop，版本来自各自的 `Cargo.toml`。每个程序包均附带 `.sha256` 校验文件，包内保留使用说明、版本信息和第三方许可。macOS 和 Linux 当前提供便携二进制包，Windows 提供便携包和可选的当前用户安装脚本。

## 触发构建

- 向 `main` 推送提交，或创建、更新拉取请求时，自动构建全部四类产物。
- 在仓库的 **Actions → 跨平台打包 → Run workflow** 中选择分支，也可手动构建。
- 推送 `v<version>` 标签时构建，并在全部平台打包成功后发布 GitHub Release 附件。标签必须与 `Cargo.toml` 中的版本一致，否则打包会失败。

普通分支、拉取请求和手动运行只上传构建产物，不自动创建版本标签或 GitHub Release。同一分支的新提交会取消过时的打包任务；版本标签的打包任务保留到完成。

## 下载程序包

1. 登录 GitHub，进入对应仓库的 **Actions → 跨平台打包**。
2. 打开所需提交的运行记录，在页面的 **Artifacts** 区域选择平台，或使用任务摘要中的“下载构建产物”链接。
3. 下载的 Actions 产物是外层 ZIP，解压后得到产品程序包及其 `.sha256`。再完整解压产品程序包后运行。

产物名称为 `<仓库名>-windows-x64`、`<仓库名>-macos-x64`、`<仓库名>-macos-arm64`、`<仓库名>-linux-x64`，保留 30 天。正式版本可从仓库的 **Releases** 页面下载附件，不受本工作流的 30 天产物保留设置影响。GitHub 的下载步骤见 [构建产物下载说明](https://docs.github.com/en/actions/managing-workflow-runs-and-deployments/managing-workflow-runs/downloading-workflow-artifacts)。

运行程序不需要 Rust。Windows 的可选开始菜单安装需要 Python 3.9 或更新版本，完整步骤见 [Windows 用户安装](windows-installation.md)。程序包未签名，macOS 未公证；Linux 需要对应系统动态库，Oracle 及数据库备份工具仍按各产品说明准备。

## 维护与排错

- CI 与发布构建统一使用已验证的 Rust `1.95.0` 和固定的运行器版本；`Cargo.lock` 保持依赖锁定。项目的最低 Rust 版本仍由 `Cargo.toml` 独立声明。
- Windows 打包使用 PowerShell 调用 `scripts/package.ps1`，初始化 MSVC 并明确指定链接器，防止 Git Bash 中同名的 GNU `link.exe` 抢占。发布标签通过 `-ExpectedTag` 传入同一套版本校验。
- 打包显式传入 `--no-install`，不会尝试在托管运行器上创建开始菜单或注册文件打开方式。
- 上传步骤缺少程序包时直接失败；已压缩的文件不再重复压缩。每个平台的摘要列出文件名、大小与下载链接。
- 重跑同一版本标签时，会创建缺失的 GitHub Release，或更新已有版本的同名附件，避免重复创建版本导致失败。
- “Rust”检查和“跨平台打包”是独立工作流。打包成功说明程序包已经生成，功能测试、SQLShop 专用数据库联测和实际桌面运行的验证范围应分别查看对应记录。

共同开发与打包约定见 [架构说明](architecture.md)。运行器的可用版本见 [GitHub 官方运行器清单](https://github.com/actions/runner-images)，产物参数见 [上传构建产物文档](https://github.com/actions/upload-artifact)。
