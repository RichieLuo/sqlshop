# Shop 项目约定

SQLShop、LinkShop、RPShop 使用相同的桌面入口、目录职责和发布流程。每个仓库均可独立克隆、构建和打包，不依赖其他 Shop 仓库的位置。

## 代码边界

```text
src/main.rs          声明桌面模块，调用 app::run()
src/app.rs           创建窗口、图标与启动选项
src/cli.rs           命令行操作（有命令行功能的项目）
src/shell_open.rs    共用的 GUI 文件参数、Windows 单实例与文件转发
src/branding.rs      运行时图标与品牌资源
src/branding_icon.rs Windows 图标像素生成，供 build.rs 使用
src/lib.rs           对外暴露业务模型、服务与存储模块
src/ui/mod.rs        界面状态、eframe 生命周期与分区组合
src/ui/*.rs          按功能划分的界面、控件与界面测试
tests/              业务集成测试
assets/             内嵌图像、字体及其许可
scripts/            开发、检查、打包与专项验证入口
docs/               功能范围、验证记录与维护说明
```

`main.rs` 不放业务逻辑。`app.rs` 管窗口启动，`ui/` 将用户操作交给库中的业务模块；库不依赖桌面入口或 UI 模块。界面分区使用正常的 Rust 子模块声明，测试放在所属模块旁。各项目的数据库、HTTP、原生 RP 格式等业务模块继续使用自己的名称，不为目录对称而强行抽象。

`branding.rs` 可保留各平台所需的图标处理；SQLShop 的 macOS Dock 和 Windows 窗口行为仍由自己的平台适配实现。现有配置目录、文件格式和命令行参数保持兼容。

三仓库的 `shell_open.rs` 保持一致。Windows GUI 在当前用户的登录会话中按产品保持单实例；再次启动将文件请求转发给现有 GUI，并请求恢复窗口和聚焦，不重复打开会写入同一配置的实例。SQL 文件只载入编辑器，SQLite 交给已有本地数据库入口；LinkShop 使用现有 Postman 导入流程；RPShop 沿用未保存文档确认。纯命令行操作独立运行，不进入 GUI 单实例转发。

## 开发与检查

三个项目的最低 Rust 版本均为 1.89，使用 Rust 2024 edition；依赖由各自的 `Cargo.lock` 固定，应用的发布版本独立管理。开发 profile 禁用增量编译；release profile 使用 thin LTO 和 strip。

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 run
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 check
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 verify
```

任务语义一致：`check` 检查所有 target；`clippy` 执行 lint 并拒绝 warning；`test` 运行默认测试；`fmt` 检查格式；`verify` 依次执行 fmt、clippy、test。`build` 和 `release` 只编译，产物仍在 Cargo target 目录。`run` 后可传应用参数。RPShop 另保留原有 `demo`、`screenshot` 别名。

Windows 脚本共用 `common.ps1`，优先选择完整的 VS 2022 C++ 工具链和 Windows SDK；其他系统或已配置的开发 shell 可直接使用 Cargo。三个项目均使用 Cargo 的注册表配置，不再由脚本强制指定镜像；旧 LinkShop 的 `-CratesIo` 参数继续接受。

## 打包契约

打包需 Python 3.9+，只使用标准库。Windows 入口负责准备 MSVC 环境，随后调用与 CI 相同的 `package.py`：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1 -OutputDirectory .\dist\candidate
```

其他系统也可直接执行：

```sh
python3 scripts/package.py
python3 scripts/package.py --target aarch64-apple-darwin
python3 -m unittest discover -s scripts/tests -v
```

默认使用 `rustc -vV` 的 host target。显式 target 必须具有对应 Rust 标准库和本机/交叉编译工具链。脚本执行 `cargo build --locked --release --target <target>`；Windows 使用静态 MSVC CRT。`--skip-build`（PowerShell 的 `-SkipBuild`）仅封装已存在的对应 target 产物，不重新保证其编译选项。Cargo 配置或 `CARGO_TARGET_DIR` 指定的 target 目录受支持。

`Cargo.toml` 的 `[package.metadata.shop]` 声明 `product-name`。版本从 Cargo metadata 获取，输出为：

```text
dist/<Product>-<version>-<os>-<arch>/
  <crate-name>[.exe]
  shop-launcher.exe               Windows：稳定入口使用的轻量启动器
  install.ps1 / windows_install.py / create-shortcut.ps1  Windows 用户安装入口
  windows_desktop.py / launch-desktop.ps1  Windows 真实桌面安装代理
  使用说明.txt
  package.json
  THIRD-PARTY-NOTICES.tsv
  licenses/
  LICENSE                         仓库具有项目许可证时
dist/<Product>-<version>-<os>-<arch>.zip[.sha256]     Windows
dist/<Product>-<version>-<os>-<arch>.tar.gz[.sha256]  macOS / Linux
```

平台名使用 `windows`、`macos`、`linux`；架构名使用 `x64`、`arm64`。压缩包内保留同名顶层目录；程序名统一使用小写 crate 名。`package.json` 记录版本、target、打包时间和程序 SHA-256，Windows 另记录启动器文件名与 SHA-256；旁边的 `.sha256` 校验整个压缩包。使用说明来自 `docs/portable-readme.txt`，自动添加版本与平台。Windows 打包同时用 Rust 标准库编译 GUI 子系统启动器，不增加 Cargo 依赖。

许可清单覆盖指定平台解析出的 Cargo 依赖图（包括构建/测试依赖及本地 vendor 补丁），复制可找到的许可文件；内嵌资产中的许可也一并收集。清单不能代替人工核查第三方资产的分发要求。

已有同名目录、压缩包或校验文件时拒绝覆盖；用新的输出目录生成候选包。构建或准备失败不会留下半成品发布目录。不会另外覆盖一个没有版本号的 `dist/<app>.exe`。

Windows 首次使用 `package.ps1 -Install` 或 `scripts/install.ps1 -PackageDirectory <完整包目录>` 安装开始菜单与“打开方式”。之后本仓库每次成功完成本机 Windows 打包都会更新现有安装的稳定入口；`-NoInstall` / `--no-install` 可跳过，CI 和跨平台打包也会跳过。安装只使用当前用户目录和 HKCU 注册表，不要求管理员。启动器读取原子替换的 `current.path`，启动保留许可文件的不可变版本目录；已运行的实例保留会话，关闭后重新启动才使用新版。不会覆盖、清理或强行结束旧版本。文件默认打开方式由 Windows 设置或“打开方式”中的用户选择决定，脚本不修改 `UserChoice` 或已有扩展名默认值。完整步骤和注册范围见 [Windows 用户安装](windows-installation.md)。

Windows 安装和自动刷新通过真实 Explorer 桌面对象启动隐藏的 Python 代理，避免从 MSIX 工具启动时把开始菜单与 HKCU 写入工具自己的虚拟视图。安装状态只在代理中判断；代理校验开始菜单目录句柄的真实路径后执行安装，并返回退出码、JSON 结果和原始错误输出。交换文件位于用户目录下的临时目录，完成后清理，不使用会被虚拟化的 AppData 临时目录。代理不提权、不注册任务计划，也不修改宿主工具的 manifest。

## CI 与发布

`ci.yml` 在 Windows、macOS、Linux 运行格式检查、clippy、默认测试和打包契约测试。SQLShop 额外保留原有真实数据库集成任务。`release.yml` 在 Windows x64、macOS x64/arm64、Linux x64 调用相同的打包脚本，并上传压缩包和校验文件供检查。

只有推送 `v<version>` tag 才创建 GitHub Release，tag 必须与 Cargo 版本一致；普通分支、PR 和手动运行只构建产物。包为未签名的便携程序，Windows 附带可选的当前用户安装脚本；macOS 为二进制 tar.gz，Linux 需要目标系统的动态库；macOS 未签名或公证。各平台是否构建及启动成功，以实际 CI 和目标机器验证为准。

修改通用脚本、工作流或本约定时，应将相同改动同步到另外两个仓库，并运行打包契约测试，避免三份独立入口逐渐分叉。

## 模块维护

详细拆分结果见 [本轮结构调整](refactoring.md)。优先按模型、持久化、协议处理、操作协调和视图职责拆分，测试按行为分组，共用 fixture 留在对应测试入口。公共业务导入路径通过显式重导出保持兼容；实现细节继续限制在所属模块或 UI 内，不为拆文件扩大为公共 API。

`python scripts/check-structure.py` 统计 `src`、`tests`、`scripts`、`examples` 中的自有代码。CI 拒绝超过 1,200 行或 128 KiB 的代码文件；此上限是防止回退的检查线，增加新职责时应更早拆分。vendor 上游代码、构建产物和资产数据不参与此检查。`.editorconfig` 与 `.gitattributes` 固定缩进、UTF-8 和源文件换行约定。
