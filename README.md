<p align="center">
  <img src="assets/branding/sqlshop-lynx-logo.png" alt="SQLShop 猞猁标志" width="560">
</p>

# SQLShop

## 目录

- [项目简介](#项目简介)
- [主要功能](#主要功能)
- [快速开始](#快速开始)
- [Windows 安装与文件打开](#windows-安装与文件打开)
- [开发与打包](#开发与打包)
- [本地数据与限制](#本地数据与限制)
- [项目结构与文档](#项目结构与文档)
- [许可证](#许可证)

## 项目简介

SQLShop 是使用 Rust 编写的本地桌面数据库工作台，提供 SQL 编辑、表数据管理、Redis 键管理，以及 SQLite 文件和 Oracle 查询文档。界面使用简体中文，支持跟随系统、浅色和深色外观；无需注册产品账号，连接时使用数据库自身的认证方式。

## 主要功能

| 功能 | 说明 |
| --- | --- |
| 连接与查询 | SQL 与 Redis 连接、连接分组、对象浏览、独立查询会话与查询库。 |
| SQL 编辑与结果 | 高亮、查找替换、格式化、目标元数据补全、多结果集、服务器消息与 CSV 导出。 |
| 表数据与对象设计 | 分页筛选、多格暂存、事务提交、CSV 导入、表结构及受支持对象的可视化编辑。 |
| 管理工具 | 执行计划、会话与锁、账号权限、备份恢复、表比较、SQL 调度与外键关系图。 |
| Redis 键管理 | 增量扫描、命名空间、类型化值编辑、TTL 与受限命令控制台。 |
| 本地工作区 | SQL 文件、命名查询、草稿与历史恢复，以及 Windows 单实例文件转发。 |

<details>
<summary>功能细节与操作说明</summary>

**连接、对象与查询工作区**

- 统一管理 SQL 与 Redis 连接，支持连接分组、测试连接、目录刷新，以及按连接、数据库和模式浏览对象。单击选择对象，双击打开文档；浏览其他数据库不会改变已有查询的执行目标。
- 每个 SQL 查询标签使用独立会话，保留自己的数据库、模式、临时表、变量和手动事务；同一服务器上的多个查询可以并行。切换目标、结束会话和关闭未保存文档时保留确认流程。
- 多标签编辑器支持语法高亮、查找替换、格式化与压缩、根据绑定目标的元数据补全，以及只读折叠视图。可执行选区、全文或光标所在语句，提供停止和事务操作。
- 查询结果提供多个结果集、服务器消息、耗时与受影响行数；区分 `NULL` 与空字符串，支持单元格复制、区域复制和 CSV 导出。
- 查询库支持命名保存、更新、另存副本、搜索和重命名，并保存连接、数据库与模式关联。重复打开同一 SQL 文件或命名查询时保留已有草稿；工作区、SQL 草稿与历史可以恢复。

**表数据与对象设计**

- 表数据页提供服务端分页、条件筛选、多列排序、末页统计、记录导航、列显隐、冻结和宽度调整。默认每页 200 行，常规翻页不自动统计全表行数。
- 支持网格内编辑、多格暂存、批量新增、删除、TSV 粘贴、CSV 列映射与预览。已有记录的修改和删除要求可用主键及受支持类型；提交使用事务，并检查原值冲突。
- 表结构页按数据库及已有结构提供字段、默认值、类型、生成列、主键、唯一约束、CHECK、复合外键与普通索引编辑。修改前预览 SQL，执行前重新核对服务器定义。
- 对象目录可浏览表、视图、函数、过程、触发器及受支持的序列和物化视图；可查看定义，或进入相应对象设计器。目录中的估计行数和大小属于参考信息。
- 刷新失败时保留上次快照并标记过期；提交状态不确定时提供草稿与服务器当前值核实流程，写入不会自动重放。

**管理工具**

“工具”菜单提供执行计划、活动会话与锁、累计查询统计、基础账号与权限管理、官方工具备份恢复、表结构与数据比较、SQL 调度和外键关系图。

表比较的每侧完整范围上限为 1,000 行；同步限于适配主键和类型的同引擎更新与插入，不删除目标独有行。备份恢复需要安装或指定相应数据库的官方工具；SQL Server 的备份文件位于服务器端，恢复只接受空或不存在的目标，不兼容 Navicat 备份格式。调度仅在应用打开时运行，失败或中断后不会自动重跑。

**Redis 键管理**

- 支持单机 Redis、密码与 ACL 用户名密码认证，可使用直连 TLS 或 SSH 隧道。连接从 DB0 开始，根据服务器实际返回的信息列出数据库，双击后切换。
- 使用 `SCAN` / `MATCH` 增量浏览，支持命名空间树与列表，最多预览 20,000 个键；扫描不是一致性快照，不预加载全库。
- String 提供文本、JSON 和十六进制视图，保存时核对原值并保留 TTL；Hash、List、Set、ZSet 提供类型化编辑。String 预览上限为 1 MiB，集合预览上限为 200 项。
- 支持新建、重命名、删除、复制键名和 TTL 设置；修改未保存时保留切换确认。命名空间删除先扫描并确认预览快照，最多接受 10,000 个键及累计 4 MiB 键名。
- 控制台支持受限的常用命令与内存历史；写命令先确认，失败后不会自动重连或重放。Stream 编辑、Cluster、Sentinel、TLS 与 SSH 同时启用、自定义证书及客户端证书尚未支持。

String 的原子写入需要服务器允许 `EVAL` 和相应内部命令，保留 TTL 需要 Redis 6 或更新版本；集合编辑不提供 SQL 事务或并发冲突合并。Redis 值草稿与命令结果不持久保存。

</details>

## 快速开始

### 使用发布包

完整解压对应平台的发布包后，运行 Windows 下的 `sqlshop.exe`，或 macOS、Linux 下的 `sqlshop`。直接运行发布包不需要 Rust；Oracle 连接和数据库备份恢复仍需要相应的外部工具。

### 从源码启动

需要 Rust 1.89 或更新版本。Windows 还需要完整的 MSVC C++ 工具链和 Windows SDK，开发脚本会尝试初始化本机工具链：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 run
```

macOS、Linux 或已配置工具链的终端可直接运行：

```sh
cargo run --locked
```

Linux 还需要桌面图形库、D-Bus 等系统依赖，安装清单可参考 [持续集成配置](.github/workflows/ci.yml)。

### 基本操作

1. 点击“新建”，选择数据源并填写地址与认证信息，测试后保存并连接。连接表单无需预选数据库。
2. 从左侧选择数据库和对象，双击表打开数据页，或新建查询并确认其执行目标。SQLite 与 Oracle 从“工具”菜单进入。
3. 编辑或运行 SQL，查看结果与消息；常用查询可保存到查询库，已有 SQL 文件可继续保存到原文件。

| 快捷键 | 操作 |
| --- | --- |
| `Ctrl/⌘+Enter` | 执行选区，无选区时执行全文 |
| `Ctrl/⌘+Shift+Enter` | 执行光标所在语句 |
| `Ctrl/⌘+S` | 保存当前 SQL 文件或命名查询 |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | 切换对象页与文档标签 |

## Windows 安装与文件打开

### 首次安装

安装需要 Python 3.9 或更新版本，并在当前用户的 Windows 桌面中运行。在仓库目录首次打包并安装：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1 -Install -OutputDirectory .\dist\installed-candidate
```

开始菜单会出现 `Shop` 文件夹及 `SQLShop` 快捷方式。已有完整包也可以通过仓库中的安装入口安装：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1 -PackageDirectory '.\dist\installed-candidate\SQLShop-0.1.0-windows-x64'
```

### 后续更新

之后在同一仓库成功完成本机 Windows 打包，会自动更新开始菜单和文件打开命令的固定入口：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1 -OutputDirectory .\dist\next-candidate
```

每次使用新的输出目录，已有同名发布包不会覆盖。加 `-NoInstall` 可只打包；持续集成和交叉编译不会更新本机安装。已打开的窗口保留当前会话，关闭后重新打开才使用新版本。

### 文件关联

| 文件类型 | 打开行为 |
| --- | --- |
| `.sql` | 载入编辑器，保留已有同文件草稿；不自动连接或执行。文件需为 UTF-8，最大 64 MiB；超过 1 MiB 时分段编辑，保存保留全文 |
| `.sqlite`、`.sqlite3`、`.db`、`.db3` | 通过文件打开入口载入已有且头部有效的 SQLite 数据库，不创建缺失文件 |

安装会注册文件的“打开方式”，保留现有默认应用；需要默认使用 SQLShop 时，在 Windows“默认应用”或文件右键菜单中选择一次。`.db` 是通用扩展名，文件本身仍需为 SQLite 格式。

同一 Windows 用户登录会话中，重复启动会将文件请求转交已有窗口并恢复、聚焦窗口。也可以从源码传入文件路径：

```sh
cargo run --locked -- --open "./queries/example.sql"
```

安装位置、版本保留和默认应用设置见 [Windows 用户安装说明](docs/windows-installation.md)。

## 开发与打包

### 开发检查

Windows 开发环境中运行：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 verify
python -m unittest discover -s scripts/tests -v
python scripts/check-structure.py
```

也可在已配置的环境中分别运行 `cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings` 和 `cargo test --locked`。macOS、Linux 的 Python 命令使用 `python3`。

<details>
<summary>真实数据库联测</summary>

真实数据库联测默认跳过。使用本项目的专用 Docker 实例前，复制 `.env.test.example` 为 `.env.test` 并填写测试密码，然后运行：

```sh
python3 scripts/test-docker.py up
python3 scripts/test-docker.py test-all
python3 scripts/test-docker.py status
python3 scripts/test-docker.py down
```

脚本使用独立的 MySQL、PostgreSQL、SQL Server 测试数据库，默认本机端口分别为 `23306`、`25432`、`21433`。联测会创建和修改测试数据，`down` 会删除这些测试容器及数据卷；请仅使用专用测试实例。脚本也提供 `test`、`test-browser`、`test-edit`、`test-query`，可以指定服务名进行单独检查。

</details>

<details><summary>字体、界面截图与平台图标检查</summary>

Linux 可通过 `SQLSHOP_FONT` 指定中文字体。原生截图使用示例数据，写入图片后退出：

```sh
SQLSHOP_FONT=/path/to/chinese-font.ttf cargo run --locked
SQLSHOP_SCREENSHOT=/path/to/sqlshop.png cargo run --locked
```

截图可额外设置 `SQLSHOP_SCREENSHOT_THEME=light` 或 `dark`、`SQLSHOP_SCREENSHOT_SIZE=1920x1080`，以及 `SQLSHOP_SCREENSHOT_VIEW`。常用视图包括 `browser`、`query`、`results`、`cell-edit`、`catalog`、`ssh`、`sqlite`、`oracle`、`structure`、`compare` 和 `jobs`；完整视图入口见 [界面启动代码](src/ui/startup.rs)。截图模式不读取用户连接、查询库或工作区，也不连接数据库。

macOS 可显式运行原生 Dock 图标检查：

```sh
SQLSHOP_TEST_NATIVE_ICON=1 cargo test --locked --test macos_icon
```

Windows 真实桌面安装代理与启动器的可选检查见 [Windows 用户安装说明](docs/windows-installation.md#验证)。

</details>

### 发布打包

Windows 发布打包：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1 -OutputDirectory .\dist\candidate
```

macOS、Linux 打包：

```sh
python3 scripts/package.py
```

打包需要 Python 3.9 或更新版本，只使用标准库。版本取自 `Cargo.toml`，Windows 生成带版本号的目录、ZIP 和 `.sha256`，macOS、Linux 生成 tar.gz；包内包含使用说明、`package.json`、第三方许可清单与许可文件。Windows 发布包使用静态 MSVC 运行库。

`dev.ps1 release` 只编译，不生成发布包。

### 持续集成

普通分支推送触发持续集成；只有推送与 Cargo 版本一致的 `v<version>` 标签才创建 GitHub Release。发布包未签名，macOS 未公证，Linux 需要相应系统动态库。完整约定见 [架构与打包说明](docs/architecture.md)。

## 本地数据与限制

### 本地数据

数据保存在系统应用配置目录，Windows 通常为 `%APPDATA%\sqlshop\SQLShop\config`，实际路径由 `directories::ProjectDirs` 定位。

| 文件 | 内容 |
| --- | --- |
| `connections.json`、`redis-connections.json` | 连接配置 |
| `catalog.json` | 命名查询、连接分组及关联 |
| `workspace.json` | 可恢复的工作区、SQL 草稿与历史 |
| `jobs.json` | 调度配置与 SQL |
| `credentials.enc`、`credentials.key` | 加密凭据与本机密钥 |

数据库密码和 SSH 口令不写入连接 JSON。启用“记住密码”时使用 AES-256-GCM 加密，凭据文件和密钥保存在同一应用目录；迁移时需要同时保留两者，能够读取两者的程序仍可解密。查询、历史和调度 SQL 以明文保存。配置或凭据读取失败时，程序会报告错误并保护原文件。

### 支持范围

| 数据源 | 主要用途 | 支持范围 |
| --- | --- | --- |
| MySQL、PostgreSQL、SQL Server | 连接管理、对象浏览、独立查询、表数据编辑与设计 | 主工作区 |
| Redis | 数据库切换、键浏览、类型化值编辑与命令控制台 | 独立键管理页面 |
| SQLite | 打开或新建数据库文件、对象浏览与 SQL 查询 | 独立文档 |
| Oracle | 连接、对象定义、SQL 查询、手动事务与停止操作 | 独立文档，需要本机 Oracle Instant Client |

SQLite 和 Oracle 尚未覆盖主工作区的全部网格与设计器能力。详细功能状态见 [功能清单与实施记录](docs/sqlshop-rebuild-plan.md)。

**认证、SSH 与 TLS**

关系数据库支持直连 TLS 与 SSH 隧道，SSH 支持密码、OpenSSH 私钥及加密私钥口令。默认通过 `~/.ssh/known_hosts` 校验主机密钥，也可填写已核实的 SHA256 指纹；暂不支持 SSH Agent、跳板链、交互式多因素认证、主机证书或读取 `~/.ssh/config`。

SQL Server 支持用户名密码、Entra 令牌和命名实例发现，Windows 构建另支持 SSPI / Windows 凭据。令牌不自动续期；SSH 场景需填写确定的 TCP 端口，不能使用 UDP 命名实例发现。macOS、Linux 的 Kerberos 尚未接入。

数据库 TLS 与 SSH 分别配置。MySQL、PostgreSQL 的隧道连接使用本机转发地址校验数据库证书名称，SQL Server 保留源数据库主机名；SSH 服务器到数据库这一段是否加密由数据库 TLS 决定。当前尚未提供自定义 CA 与客户端证书配置。

### 使用限制

- 普通 SQL 查询每个结果集最多展示前 1,000 行；查询 CSV 只包含已展示或选中的行。完整表 CSV 按当前筛选与排序逐页读取，要求主键及受支持列，不保证导出期间的一致性快照。
- 普通查询结果只读。表数据编辑取决于主键、列类型和引擎；视图、生成列、受限类型及 MySQL 非 InnoDB 表等不提供相同的写入能力。表的暂存修改不持久保存。
- CSV 中未加引号的空字段表示 `NULL`，`""` 表示空字符串，文字 `NULL` 仍是字符串。CSV 导入上限为 64 MiB；它不包含完整类型、约束和一致性快照，不能替代数据库备份。
- SQL 执行确认用于核对目标与文本，不是权限隔离。直接执行的 SQL 遵循数据库自身的事务规则，断线前的语句可能已经提交；程序不会自动重放不确定的写入。
- 当前语句拆分不解释 `GO`、`DELIMITER` 等客户端指令；不完整引用、过程体和无法可靠划分的语句需显式选择完整 SQL 执行。
- 数据库专有类型、复杂 DDL、跨引擎转换和部分管理操作仍有范围限制，完整说明见 [功能清单](docs/sqlshop-rebuild-plan.md)。

## 项目结构与文档

| 路径 | 职责 |
| --- | --- |
| `src/main.rs`、`src/app.rs` | 桌面入口与窗口选项 |
| `src/shell_open.rs` | 文件参数、单实例与窗口转发 |
| `src/ui/` | 界面、操作协调与界面测试 |
| `src/db/` | 关系数据库驱动及目录、设计、编辑能力 |
| `src/redis_database/` | Redis 连接、键操作与协议处理 |
| `src/storage.rs` | 本机配置与加密凭据 |
| `src/workspace.rs` | 工作区、SQL 文件、草稿与历史 |
| `scripts/` | 开发、打包、安装与数据库联测 |

- [架构与打包说明](docs/architecture.md)：三个 Shop 项目的共同目录、开发与发布约定。
- [Windows 用户安装](docs/windows-installation.md)：开始菜单、自动更新、文件类型与默认应用。
- [功能清单与实施记录](docs/sqlshop-rebuild-plan.md)：功能范围、尚存差异与验收记录。
- [模块拆分说明](docs/refactoring.md)：各模块职责与验证记录。
- [品牌资源说明](docs/branding.md)：界面图标和文档品牌图片。

界面使用 egui / eframe，MySQL、PostgreSQL 使用 SQLx，SQL Server 使用 Tiberius，SSH 使用 Russh。

## 许可证

仓库尚未声明项目许可证；第三方组件及许可随发布包提供，见 `THIRD-PARTY-NOTICES.tsv` 和 `licenses/`。
