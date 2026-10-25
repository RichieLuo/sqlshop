# Shop 项目约定

SQLShop、LinkShop、RPShop 使用相同的桌面入口、目录职责和发布流程。每个仓库均可独立克隆、构建和打包，不依赖其他 Shop 仓库的位置。

## 代码边界

```text
src/main.rs          声明桌面模块，调用 app::run()
src/app.rs           创建窗口、图标与启动选项
src/cli.rs           命令行操作（有命令行功能的项目）
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
  使用说明.txt
  package.json
  THIRD-PARTY-NOTICES.tsv
  licenses/
  LICENSE                         仓库具有项目许可证时
dist/<Product>-<version>-<os>-<arch>.zip[.sha256]     Windows
dist/<Product>-<version>-<os>-<arch>.tar.gz[.sha256]  macOS / Linux
```

平台名使用 `windows`、`macos`、`linux`；架构名使用 `x64`、`arm64`。压缩包内保留同名顶层目录；程序名统一使用小写 crate 名。`package.json` 记录版本、target 和程序 SHA-256；旁边的 `.sha256` 校验整个压缩包。使用说明来自 `docs/portable-readme.txt`，自动添加版本与平台。

许可清单覆盖指定平台解析出的 Cargo 依赖图（包括构建/测试依赖及本地 vendor 补丁），复制可找到的许可文件；内嵌资产中的许可也一并收集。清单不能代替人工核查第三方资产的分发要求。

已有同名目录、压缩包或校验文件时拒绝覆盖；用新的输出目录生成候选包。构建或准备失败不会留下半成品发布目录。不会另外覆盖一个没有版本号的 `dist/<app>.exe`。

## CI 与发布

`ci.yml` 在 Windows、macOS、Linux 运行格式检查、clippy、默认测试和打包契约测试。SQLShop 额外保留原有真实数据库集成任务。`release.yml` 在 Windows x64、macOS x64/arm64、Linux x64 调用相同的打包脚本，并上传压缩包和校验文件供检查。

只有推送 `v<version>` tag 才创建 GitHub Release，tag 必须与 Cargo 版本一致；普通分支、PR 和手动运行只构建产物。包为未签名的便携程序，macOS 为二进制 tar.gz，Linux 需要目标系统的动态库，均非安装包；macOS 未签名或公证。各平台是否构建及启动成功，以实际 CI 和目标机器验证为准。

修改通用脚本、工作流或本约定时，应将相同改动同步到另外两个仓库，并运行打包契约测试，避免三份独立入口逐渐分叉。

## 模块维护

详细拆分结果见 [本轮结构调整](refactoring.md)。优先按模型、持久化、协议处理、操作协调和视图职责拆分，测试按行为分组，共用 fixture 留在对应测试入口。公共业务导入路径通过显式重导出保持兼容；实现细节继续限制在所属模块或 UI 内，不为拆文件扩大为公共 API。

`python scripts/check-structure.py` 统计 `src`、`tests`、`scripts`、`examples` 中的自有代码。CI 拒绝超过 1,200 行或 128 KiB 的代码文件；此上限是防止回退的检查线，增加新职责时应更早拆分。vendor 上游代码、构建产物和资产数据不参与此检查。`.editorconfig` 与 `.gitattributes` 固定缩进、UTF-8 和源文件换行约定。
