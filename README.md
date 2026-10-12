<p align="center">
  <img src="assets/branding/sqlshop-lynx-logo.png" alt="SQLShop · 猞猁 Logo" width="560">
</p>

# SQLShop

纯 Rust 编写的本地桌面数据库工具，第一版支持 MySQL、PostgreSQL 和 Microsoft SQL Server（MSSQL）。无需注册产品账号，不依赖云端服务，也没有连接数或付费功能限制。数据库连接仍使用数据库自己的用户名和密码。

## 启动

需要 Rust 1.89 或更新版本。在项目目录运行：

```sh
cargo run --locked
```

首次启动点击左侧“新建”，选择数据库类型，填写服务器地址、端口、用户名和密码，无需填写数据库名称。可以先“测试连接”，再“保存并连接”。连接后展开左侧连接查看数据库列表，点击要使用的数据库，再展开查看表和视图。

默认启用 TLS 并验证服务器证书。本机未配置 TLS 的测试数据库可关闭 TLS；使用自签名证书的服务器可以勾选“信任服务器证书”。首版暂未提供自定义 CA 和客户端证书配置。

## 第一版功能

- 创建、编辑、删除及保存连接，支持多个连接配置，当前同时使用一个活动会话。
- 连接服务器、展开数据库列表、选择数据库、断开和刷新目录。
- 右键连接节点可连接、断开、编辑、删除或刷新数据库列表；右键当前数据库可刷新表和视图。左侧顶部保留新建连接和搜索，双击连接也可连接服务器。
- 通过 SSH 隧道连接三种数据库，支持 SSH 密码、私钥及加密私钥口令。
- 浏览数据库中的表与视图，按名称筛选；单击只选中对象，双击打开标签页并加载前 200 行。同一连接、数据库及 schema 中的同一张表复用已有标签页，保留 SQL 和查询结果。
- 多标签 SQL 编辑；`⌘ + Enter` / `Ctrl + Enter` 优先执行选中内容，否则执行整段 SQL。
- 标签页单行横向滚动，点击 `×` 或中键关闭；右键可关闭其他标签或右侧标签。关闭最后一个标签后自动创建空白查询，关闭后的 SQL 和结果不保存。执行期间暂不可关闭标签。
- SQL 编辑区与结果区之间可上下拖动分隔线，双击分隔线恢复默认高度。
- 未执行的查询标签只显示 SQL 编辑区，使用全部可用高度；开始执行后才显示结果区和分隔线，各标签独立保留这个状态。
- 查看多个结果集，区分 NULL 与空字符串，复制单元格，导出当前结果集为 CSV。
- 后台执行数据库操作，直连超时 15 秒，SSH + 数据库连接总超时 30 秒，查询超时 60 秒。
- 中文界面，默认跟随系统外观；右上角可选择“跟随系统 / 浅色 / 深色”。

查询只保留每个结果集的前 1000 行，超出时在界面提示。导出的 CSV 同样只包含已展示的行。这是当前版本的内存保护，可在 `src/db/mod.rs` 调整 `ROW_LIMIT`。流会继续读取到结束，保证下一次查询可以复用会话；请用 LIMIT / TOP 控制大查询。最多接受 32 个结果集。

## 本机存储

连接配置由 `directories` 定位到系统应用配置目录，文件名为 `connections.json`。macOS 通常位于 `~/Library/Application Support/dev.sqlshop.SQLShop/`；以 `ProjectDirs` 的实际返回路径为准。Unix 上配置文件权限为 `0600`，采用临时文件替换保存。

密码不写入 `connections.json`。勾选“记住密码”时，数据库密码和 SSH 密码 / 私钥口令使用 AES-256-GCM 加密，保存在同一应用目录的 `credentials.enc`，随机密钥保存在 `credentials.key`。Unix 上这两个文件权限均为 `0600`。不使用系统钥匙串，不要求系统登录密码或授权；私钥内容不复制到配置文件中。能够读取密钥和凭据文件的程序仍可以解密，因此这两个文件都应作为敏感文件保存。关闭相应的“记住”选项并保存后，该密码只留在当前进程内，重启后需重新输入。

旧版连接配置可以继续读取，但不会自动访问钥匙串迁移密码。首次使用新版时重新填写一次数据库密码及所需 SSH 口令，勾选“记住”后保存即可。凭据文件损坏或密钥丢失时会报告错误，禁止覆盖原凭据文件；备份或迁移记住的密码时需同时保留 `credentials.enc` 和 `credentials.key`。

配置损坏时应用显示错误，并禁止覆盖保存；修复或备份移走损坏文件后重启即可。

中文字体优先使用本机 PingFang、微软雅黑或 Noto Sans CJK。Linux 如未安装这些字体，可设置：

```sh
SQLSHOP_FONT=/absolute/path/to/chinese-font.ttf cargo run --locked
```

Linux 编译还需要桌面图形库及 D-Bus 开发包；本项目目前在 macOS 上完成编译和窗口启动验证，Windows / Linux 尚未验证。

## SSH 隧道

连接编辑窗口同时展示数据库类型、数据库参数和 SSH 配置。在“SSH 隧道”区域勾选“通过 SSH 隧道连接”。例如数据库运行在跳板机本机：

| 配置 | 示例 |
| --- | --- |
| 数据库主机 | `127.0.0.1`，从 SSH 服务器视角访问 |
| 数据库端口 | MySQL `3306` / PostgreSQL `5432` / SQL Server `1433` |
| SSH 主机 / 端口 | 服务器公网地址 / `22` |
| SSH 用户名 | SSH 系统用户，与数据库用户分开 |
| SSH 认证 | 密码，或私钥文件及其解密口令 |

私钥和 known_hosts 路径支持 `~/`。默认使用 `~/.ssh/known_hosts` 校验服务器主机密钥。如果主机尚未受信任，“测试连接”的错误信息会给出 SHA256 指纹；核实该指纹后，在“SSH 主机校验”中填写，随后重新测试。填写指纹后采用指纹校验，不会自动修改系统 known_hosts，也不会自动接受未知或变化的主机密钥。

测试连接会真正通过 SSH 尝试登录数据库。连接失败、超时、断开、切换连接及测试结束后会关闭隧道。转发只监听随机的本机回环端口，并且仅接受所属数据库会话的一次连接。无需系统 `ssh` 命令，不会启动外部转发进程。旧版没有 SSH 字段的连接配置仍按直连读取。

数据库 TLS 与 SSH 是两层独立加密，不会因为启用了 SSH 而自动关闭 TLS 或跳过证书校验。当前 SQLx 驱动在 MySQL / PostgreSQL 的隧道连接中以本机转发地址 `127.0.0.1` 校验数据库 TLS 主机名；证书不包含此地址时严格校验会失败。可以使用符合该名称的证书，或按实际部署明确选择关闭数据库 TLS / 信任服务器证书。SQL Server 保留原数据库主机名用于 TLS 校验。SSH 加密覆盖客户端至 SSH 服务器这一段，SSH 服务器至数据库这一段是否加密由数据库 TLS 决定。

当前 SSH 支持普通主机公钥、密码认证及 OpenSSH 私钥文件认证。暂不支持 SSH Agent、跳板链、多因素交互认证、SSH 主机证书和读取 `~/.ssh/config`。

私钥字段旁的“选择文件…”打开系统文件选择框，默认定位到当前文件所在目录或 `~/.ssh`，支持无扩展名私钥。选择后自动填写完整路径；取消选择保留原值。不要选择 `.pub` 公钥文件。

数据库 TLS 设置集中在同一窗口底部，可直接勾选“信任服务器证书（跳过证书校验）”。该选项控制数据库证书；SSH 主机密钥由 known_hosts 或服务器指纹校验。宽窗口中数据库和 SSH 参数并排展示，窄窗口自动改为上下排列；测试连接和保存按钮始终位于滚动表单之外。切换数据库类型时更新默认端口和用户名，保留已填写的自定义值。数据库名称在连接成功后的目录树中选择。

## 连接服务器与选择数据库

数据库名称不再出现在连接表单中。连接成功后程序读取服务器的数据库目录，点击左侧数据库节点后加载该库的表和视图。SQL 编辑区始终显示当前连接与选中的数据库，查询结果保留执行时的来源信息。MySQL / SQL Server 的 SQL 中手动执行 USE 后也会同步当前数据库和表目录。

MySQL 连接服务器时不指定数据库，通过 information_schema.SCHEMATA 获取账号可见的数据库列表，选择时执行经过标识符转义的 USE。目录中的库名和表名统一转换为 UTF-8 文本，避免驱动将二进制排序规则的名称显示成十六进制。SQL Server 使用登录账号的默认数据库初始化，再读取 sys.databases，并通过 USE 切换。数据库访问权限不足或库已不存在时，选择失败会报告错误并保留原会话；连接故障及超时仍会关闭连接。

PostgreSQL 协议必须连接到一个具体数据库才能读取服务器目录，程序会自动尝试 postgres、与用户名同名的数据库和 template1；旧连接配置中的数据库名作为额外的初始化候选保留。若该账号对这些库都没有 CONNECT 权限，需要由管理员开放一个初始化库的访问权限。读取数据库列表后，每次选择数据库会在后台建立目标库的新会话，成功后替换原会话，失败时保留原会话。SSH 模式下重建所需隧道也由程序处理。

## 当前边界

这是一版可继续扩展的 MVP，尚未实现表格直接编辑、表结构设计、SQL 自动补全、事务管理按钮和数据导入。数据库列表按账号权限返回，选择数据库时会验证访问权限；当前同时使用一个活动数据库会话。MySQL 数据库节点下直接显示表，PostgreSQL / SQL Server 按 schema 分组。

SQL 原文发送到数据库，写入语句遵循数据库自身的自动提交规则。不会自动包装为事务。查询失败或超时后关闭活动连接，重新连接后可继续操作；批处理之前的语句可能已经提交，不应直接重试整批写入。

SQL Server 首版使用 SQL Server 用户名密码认证，不支持 Windows 集成认证及命名实例自动发现，请提供 TCP 主机和端口。SQL Server 的批处理结果暂不提供受影响行数。MySQL / PostgreSQL 的零行结果暂不能保留列标题。

常见文本、整数、小数、日期时间、JSON、UUID 和二进制值会转换为展示文本。部分数据库专有类型暂显示“暂不支持的类型”。CSV 中 NULL 使用文字 `NULL` 表示，与真实文本 `NULL` 无法区分，不应作为无损备份格式。

## 项目结构

```text
src/main.rs       桌面入口
src/branding.rs   内嵌猞猁图标、界面纹理及 macOS Dock 图标
src/ui.rs         连接列表、对象目录、SQL 标签页和结果表格
src/model.rs      连接配置、结果模型、方言标识符引用和 CSV 导出
src/storage.rs    本地配置及加密凭据文件
src/worker.rs     后台工作线程、超时和连接生命周期
src/ssh.rs        SSH 认证、主机密钥校验及隧道生命周期
src/db/mod.rs     MySQL / PostgreSQL / SQL Server 驱动
src/db/values.rs  数据库值转展示文本
tests/core.rs     配置、SQL 引用、CSV 和工作线程测试
tests/ssh.rs      本机自动启动的 SSH 服务及转发、认证、清理测试
tests/database_navigation.rs  PostgreSQL 协议客户端与数据库导航测试
tests/databases.rs  真实数据库联测（默认跳过）
```

界面使用 [egui / eframe](https://docs.rs/eframe/0.33.3/eframe/)，MySQL / PostgreSQL 使用 [SQLx](https://docs.rs/sqlx/0.8.6/sqlx/)，SQL Server 使用 [Tiberius](https://docs.rs/tiberius/0.12.3/tiberius/)，SSH 使用 [Russh](https://docs.rs/russh/0.64.1/russh/)。

## 验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
```

### 本机 Docker 测试数据库

`compose.test.yml` 提供本项目独立的 MySQL 8.4、PostgreSQL 17 和 SQL Server 2022 Developer 测试实例。端口只绑定到 `127.0.0.1`。每个实例有 `sqlshop_test` 和 `sqlshop_other` 两个库，同名 `navigation_probe` 表返回不同的库名，用于验证实际切换了会话。

复制 `.env.test.example` 为 `.env.test` 并设置专用密码，再运行：

```sh
python3 scripts/test-docker.py up mysql postgres sqlserver
python3 scripts/test-docker.py test mysql postgres sqlserver
```

| 服务 | 本机端口 | 用户 |
| --- | --- | --- |
| MySQL | 23306 | root |
| PostgreSQL | 25432 | postgres |
| SQL Server | 21433 | sa |

密码读取 `.env.test` 的 `SQLSHOP_DOCKER_PASSWORD`，不会写入仓库。测试脚本把凭据注入子进程环境。SQL Server 启动后由脚本初始化测试库；其他两种数据库在首次创建容器时自动初始化。可以只传其中一个服务名进行启动或测试。

这三个实例已完成 TCP 和 SSH 隧道联测。MySQL / SQL Server 还验证了自签名证书默认被拒绝，明确启用“信任服务器证书”后可通过 TLS 直连和 SSH 连接、读取列表及选库。PostgreSQL 测试实例未配置 TLS。连接这些本机实例时，PostgreSQL 关闭数据库 TLS；MySQL / SQL Server 可以保留 TLS 并勾选“信任服务器证书”。

SQL Server 镜像需要 amd64；在 ARM Mac 上使用兼容运行，其可用性取决于 Docker Desktop 的虚拟化环境。该配置用于本机开发测试。

查看及删除本项目测试容器（删除会丢弃测试数据）：

```sh
python3 scripts/test-docker.py status
python3 scripts/test-docker.py down
```

真实数据库测试默认跳过，不会自动连接本机已有数据库。准备专用测试实例后，设置以下变量，再运行对应测试。测试只执行 SELECT 和目录查询，不写入数据。

```sh
# MySQL
export SQLSHOP_TEST_MYSQL_HOST=127.0.0.1
export SQLSHOP_TEST_MYSQL_PORT=3306
export SQLSHOP_TEST_MYSQL_USER=your_test_user
export SQLSHOP_TEST_MYSQL_DATABASE=your_test_database
export SQLSHOP_TEST_MYSQL_PASSWORD='your_test_password'
cargo test --locked --test databases mysql -- --ignored --exact
```

PostgreSQL 使用前缀 `SQLSHOP_TEST_PG_`，SQL Server 使用前缀 `SQLSHOP_TEST_MSSQL_`，字段同样是 `HOST`、`PORT`、`USER`、`DATABASE`、`PASSWORD`。分别运行 `postgres` 或 `sqlserver` 测试。每种数据库分别通过 TCP 直连及本机临时 SSH 服务转发连接，覆盖服务器数据库发现、选库与失败恢复、多结果集、中文、NULL、小数、二进制、空结果及截断后会话复用。SSH 服务使用临时生成的主机密钥及指纹验证，不读取个人 SSH 私钥。DATABASE 是测试断言的目标库，不是连接表单的输入。设置 `SQLSHOP_TEST_SECOND_DATABASE` 后还会校验两个库的同名表确实返回各自的数据。

可以单独执行原生窗口渲染测试，保存应用窗口截图后自动退出：

```sh
SQLSHOP_SCREENSHOT=/absolute/path/to/sqlshop.png cargo run --locked
```

截图时可额外设置 `SQLSHOP_SCREENSHOT_THEME=light` 或 `dark`，分别检查浅色和深色界面；普通启动仍默认跟随系统。

设置 `SQLSHOP_SCREENSHOT_VIEW=ssh` 可截图新增连接的 SSH 配置表单；截图模式不会自动连接或保存配置。

macOS 可单独检查启动第 1、10、60 帧后 Dock 实际持有的图标，确认圆角底板不会被框架覆盖。测试打开检查窗口后自动退出，普通测试默认跳过：

```sh
SQLSHOP_TEST_NATIVE_ICON=1 cargo test --locked --test macos_icon
```

设置 `SQLSHOP_SCREENSHOT_VIEW=catalog` 可截图数据库目录树的布局测试；该模式使用明确标记的示例数据，不建立真实数据库连接。
