# SQLShop 重点质量审计与 Navicat 功能对齐

日期：2026-10-23（Asia/Shanghai）。审计源码基线：`ee7842a`。下面保留修复前的发现和证据；同日已按用户要求实施修复，范围及验收见末尾。

结论：项目已经具备连接、独立查询会话、表数据编辑、对象设计和管理工具的主要入口；当前最应优先解决的是写入正确性、草稿生命周期与各模块之间的上下文差异。继续增加菜单之前，应先修复下面两个 P1 和五个 P2，并把对应场景纳入回归。

## 已确认的缺陷

### 1. [P1] 字符串超长输入被静默截断，写入仍报告成功

位置：[src/db/table_edit.rs:1013](../src/db/table_edit.rs#L1013)。网格更新、新增、CSV 导入及部分筛选共用 `parameter_expression`。

PostgreSQL 把字符串参数显式转换为带长度的目标列类型，例如 `CAST(CAST($1 AS text) AS character varying(5))`。显式转换先把值截短，随后列赋值无法再发现输入超长。数据库的这个行为符合其规范，但客户端不应悄悄引入这一步转换。[PostgreSQL 字符类型文档](https://www.postgresql.org/docs/18/datatype-character.html)

已在独立 PostgreSQL 18.3 临时实例运行源码生成的同构 SQL：

```sql
CREATE TEMP TABLE audit_text(id integer PRIMARY KEY, label varchar(5));
INSERT INTO audit_text VALUES (1, 'old');
UPDATE audit_text
SET label = CAST(CAST('abcdefgh' AS text) AS character varying(5))
WHERE id = 1;
-- UPDATE 1；实际 label 为 abcde。
INSERT INTO audit_text VALUES (2, 'abcdefgh');
-- 对照：正常报 value too long for type character varying(5)。
```

同样的 INSERT 转换也丢失尾部内容；等值筛选输入 `abcdefgh` 会错误匹配 `abcde`。这是实际数据损失，不是只影响显示的截断。SQL Server 的相邻 CAST 分支也需要检查，但本轮没有将其列为已做实库验证。

修复方向：为赋值、键匹配、旧值比较和筛选分别定义参数转换策略。文本赋值保留完整值，由目标列校验长度；不能直接删掉所有非文本类型转换。回归应覆盖更新、新增、CSV 导入、长中文与等值筛选。

### 2. [P1] “保存草稿并退出”不保存 SQLite / Oracle 的 SQL

位置：[src/ui/workspace_ui.rs:1581](../src/ui/workspace_ui.rs#L1581)、[src/ui/workspace_ui.rs:269](../src/ui/workspace_ui.rs#L269)、[src/ui/management_ui.rs:40](../src/ui/management_ui.rs#L40)。

SQLite、Oracle 被归入 `management_document()`，工作区快照通过 `workspace_tabs()` 排除这些文档；SQL 实际存放在它们的子文档中。退出确认只对 Redis 草稿和对象设计草稿禁用“保存草稿并退出”，因此这两类 SQL 没有被保存，应用仍正常退出。

触发：打开 SQLite 或 Oracle 文档 → 输入未保存 SQL → 退出 → 点击“保存草稿并退出” → 重启。快照里没有该 SQL，无法恢复。此项依据完整保存/退出代码路径确认，未跑 GUI 点击验收。

修复方向：统一所有 SQL 文档的快照与恢复协议；完成前，应对无法持久化的草稿阻止“保存草稿并退出”，提供真实可用的文件保存路径。

### 3. [P2] 打开文件或浏览对象会直接覆盖未保存 SQL

位置：[src/ui/local_database_ui.rs:167](../src/ui/local_database_ui.rs#L167)、[src/ui/oracle_database_ui.rs:355](../src/ui/oracle_database_ui.rs#L355)、[src/ui/oracle_database_ui.rs:449](../src/ui/oracle_database_ui.rs#L449)。

SQLite 点击对象名称、Oracle 点击“生成读取 SQL”都会直接替换 SQL 编辑缓冲区。Oracle“打开 SQL”还直接更新 `saved_sql`。这些分支没有检查当前 dirty 状态，也没有创建新文档或保留旧草稿。

触发：先编写未保存 SQL，再使用上述入口。旧内容即被覆盖。此项是源码确认，未做 GUI 端到端复现。

修复方向：复用主 SQL 文档的打开文件/生成查询流程，先处理未保存内容，或在独立文档中打开。

### 4. [P2] 从正常 MySQL 查询创建的调度任务在执行前失败

位置：[src/jobs.rs:359](../src/jobs.rs#L359)、[src/db/query_context.rs:26](../src/db/query_context.rs#L26)、[src/worker.rs:1001](../src/worker.rs#L1001)、[src/ui/management_ui.rs:537](../src/ui/management_ui.rs#L537)。

MySQL 查询完成后，把实际数据库名同时写入 target.schema；调度表单原样复制这个值。执行任务时不分引擎调用 `select_query_schema()`，而该方法拒绝 MySQL。结果是正常来源创建的任务尚未发送用户 SQL，就已经失败，并被失败保护暂停。

已用真实 SQLx MySQL 客户端连接本地协议 fixture 复现：`database="app"`、`schema=Some("app")`、`sql="SELECT 777"`；服务端仅收到初始化和 `USE app`，没有收到用户查询。

修复方向：复用 `QuerySession::connect` 的引擎分支；MySQL 校验 schema 与 database 一致，PG 设置 search_path，SQL Server 核对默认 schema。

### 5. [P2] PostgreSQL 任务与恢复预检遗漏目标数据库

位置：[src/jobs.rs:357](../src/jobs.rs#L357)、[src/backup.rs:325](../src/backup.rs#L325)。普通查询的对照实现：[src/worker.rs:952](../src/worker.rs#L952)。

这两个模块直接拿保存的 Profile 建立初始化连接，没有把明确选择的任务/恢复目标库放入 bootstrap Profile。若 Profile.database 为空，初始化只尝试 `postgres`、用户名同名库及 `template1`。

触发条件是账号仅能连接业务目标库，通用初始化库不可访问。普通查询可以成功，但任务和恢复预检失败。

已用真实 SQLx PostgreSQL 客户端及只允许 `app_only` 的协议 fixture 复现：任务和恢复的连接记录均为 `[postgres, test_user, template1]`；对照给临时 Profile 设置 database 后，尝试第四个库 `app_only` 并成功。这证明连接选择缺陷，不等于已经验证真实备份/恢复执行。

修复方向：连接前传入明确目标，保留账号只拥有目标库访问权的测试；不应要求用户额外开放系统库权限。

### 6. [P2] PostgreSQL JSON / XML 显示可编辑，但已有值无法提交

位置：[src/db/table_edit.rs:957](../src/db/table_edit.rs#L957)、[src/db/table_edit.rs:315](../src/db/table_edit.rs#L315)，恢复核实也使用相同类型的条件：[src/db/recovery.rs:300](../src/db/recovery.rs#L300)。

类型分类把 json / xml 当作可编辑文本，乐观检查却统一生成 `column IS NOT DISTINCT FROM CAST(... AS json/xml)`。这两个 PostgreSQL 类型没有该比较所需的等值运算符。

独立 PostgreSQL 18.3 实库复现：JSON 报 `operator does not exist: json = json`，XML 报 `operator does not exist: xml = xml`；同一行普通 UPDATE 成功。包含此类修改的混合提交会回滚，恢复核实也可能失败。

修复方向：为无等值运算符的类型定义文本快照比较策略，新值仍按真实类型校验。不能把 json 全部改成 jsonb 后比较，否则会改变重复键、空白和表示形式语义。

补充风险：同一旧值比较对 citext 使用不区分大小写的等值关系。本轮还复现了旧快照为 `foo`、服务器后来变为 `FOO`，旧草稿仍能改成 `bar` 的情况。应把类型比较语义是否符合“原始快照未变”单独纳入回归。

### 7. [P2] 格式化 / 压缩会破坏合法 PostgreSQL SQL

位置：[src/sql_editor.rs:936](../src/sql_editor.rs#L936)。

合法输入：

```sql
SELECT 'foo'
'bar';
```

两种模式均输出 `SELECT 'foo' 'bar';`。PostgreSQL 只有在相邻字符串间至少包含一个换行时，才会执行这种隐式拼接。用空格替换后语句不再合法。[PostgreSQL 词法规范](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-SYNTAX-CONSTANTS)

本轮提取当前源码的原函数并临时编译，确认两种输出；未改项目代码。修复应保留有语法意义的换行，增加格式化前后执行结果一致性测试。

## 验证结果与工程问题

| 检查 | 本轮结果 | 解释 |
| --- | --- | --- |
| `cargo test --locked` | 237 通过，0 失败，25 ignored | Windows 本机默认测试，不代表三引擎实库全部通过 |
| `cargo fmt --check` | 失败；13 文件、79 个 diff 块 | 当前 CI 明确要求该命令成功，格式门禁未满足 |
| `cargo clippy --locked --all-targets -- -D warnings` | 失败 | `src/redis_database/commands.rs:521` 触发 `clippy::type_complexity`；本机质量门禁未满足 |
| PostgreSQL 数据正确性复现 | 超长截断、筛选误匹配、JSON/XML 比较错误已确认 | 独立 PostgreSQL 18.3 临时集群，运行源码同构 SQL；不是 UI 端到端 |
| 调度与初始化连接复现 | 两个问题均确认 | 真实 SQLx 客户端 + 本地协议 fixture；不是实际备份恢复验收 |
| 格式化复现 | 两个模式均破坏换行 | 当前源码函数的临时独立程序 |
| GUI、真实 MySQL / SQL Server、SSH / TLS 完整联测 | 本轮未执行 | 本机 Docker daemon 不可用；没有操作用户业务库 |

首次测试被本机自动选中的 MSVC 14.51 头文件缺失阻挡；切换到已安装的 MSVC 2022 14.44 环境后默认测试通过。没有安装软件或修改系统配置，也没有把该环境问题归为产品缺陷。

CI 的覆盖范围与功能完成记录也存在差距：[scripts/test-docker.py:70](../scripts/test-docker.py#L70) 的 `test-all` 仅运行 databases、table_browser、table_edit、query_sessions；新增 object_designer 和 table_designer 的 12 个 ignored 测试没有被 CI 实库任务调用，其中包含表读取取消。此前手工联测记录可以保留，但不能替代持续回归。仅把套件名称加入列表也不够：现有脚本使用 `--exact`，新增测试名称并不是简单的引擎名。

可维护性方面，`src/ui.rs` 已达 10,893 行，SQL 主标签与 SQLite/Oracle 文档分别维护编辑、dirty、保存、退出和目标选择，已经出现本报告中的行为分叉。重构应优先统一这些协议和连接初始化，避免只按文件长度机械拆分。

## Navicat 参考证据及范围

本次机器安装的是 **Navicat Premium 17.2.7.0，Windows x86_64**：

- 路径：`C:\Program Files\PremiumSoft\Navicat Premium 17\navicat.exe`。
- SHA-256：`162c89e00f8ee99b668063bbfa83df2fc41ca786b1f709ff22fb53955f24d34d`。
- REA `inspect_pe_resources` Evidence：`ev_44febf38acbcd413e2edca77a8bb1cebb694c87d979c9014181a263af27dfff3`。
- 完整枚举 2,530 项资源、5,070 个资源目录项。可定位连接、查询、导入导出、调度、各引擎表设计等窗体资源。资源只证明发布包中存在这些定义，不能证明授权启用、完整行为或异常处理正确性。
- 本轮启动了 Ghidra 12.1.4 对主程序的静态分析，达到 330 秒启动期限后返回 `provider_timeout`，没有取得完整分析与伪代码。已保存失败摘要并关闭 REA 会话，不以此声称已反编译或还原 Windows Navicat 的实现，也未更改 REA 配置重跑全程序分析。

仓库已有的 ARM64 有界反编译记录属于 **macOS Navicat Premium Lite 17.3.11**，保留为历史证据；其地址不能用于本次 Windows 程序，其 Lite 授权范围也不能直接当 Premium 的范围。

功能对比结合 [Navicat 17 Windows 官方手册](https://www.navicat.com.cn/manual/pdf_manual/en/navicat_17/win_manual/navicat_en.pdf) 和当前 SQLShop 源码。未运行 Navicat 连接、编辑和保存数据，下面不宣称像素或内部行为完全一致。

## 当前功能差距与对齐顺序

| 能力 | SQLShop 当前状态 | 对齐判断 / 顺序 |
| --- | --- | --- |
| 连接、独立 SQL 会话、事务、停止查询、主对象树与文档 | 三主引擎已有完整主路径 | 先保障正确性；不应重复列为整体缺失 |
| 表格连续编辑、列显隐 / 冻结、约束设计、对象保存 | 最近一轮已实现明确范围 | 优先修复本报告的截断、类型比较与草稿问题，再扩大属性覆盖 |
| 导入 / 导出 | 以 CSV 为主；SQL 结果只导出保留的前 1000 行，完整表 CSV 有独立路径 | 与 Navicat 多格式向导有差距；可先做 JSON、XLSX 与查询结果流式导出 |
| TLS 证书 | 有 TLS 开关与服务器信任选项，缺自定义 CA / 客户端证书 | 企业内网 CA 和双向 TLS 场景有兼容性缺口；应独立补齐配置、持久化与三引擎验证 |
| 数据比较 / 同步 | 每侧完整范围最多 1000 行；同引擎 UPDATE + INSERT；不删除目标独有行 | 属于有限表同步；距离全库迁移、复杂结构同步仍有明显范围差距 |
| 调度 | 应用打开时执行 SQL 间隔 / 单次计划 | 先修 MySQL 和 PG 目标初始化；Windows Navicat 手册另提供系统任务计划集成，应用关闭后调度是后续独立能力 |
| SQLite / Oracle | 独立文档，未统一接入完整主树、网格与设计器 | 优先统一草稿与查询文档生命周期，再统一数据编辑；避免仅增加入口 |
| SQL 编辑器、可视化查询 | 有格式化、基础上下文补全、只读折叠；不推断 CTE / 派生表列；未发现可视化 Query Builder 主流程 | 先修语义保持，再补常用补全；可视化构建器单列为较大功能 |
| 关系图、模型、执行计划 | 现有外键关系浏览及基础计划展示 | 关系浏览不等于完整模型编辑；全模型和图形计划可以后置 |
| Redis | 已有键管理；集合最多显示 200 项，没有继续分页；Stream 只读 | 若 Redis 是主要用途，优先补集合游标分页与明确的类型能力说明 |

Navicat 官方手册中，多格式向导见第 13 章，SSL 配置见第 4 章，系统调度见第 14 章，可视化查询见第 8 章；各功能适用的数据库和版本不同，不据产品名称推断所有组合都支持。当前官网功能矩阵还可能覆盖较新版本，不能作为本机 17.2.7 每项能力的唯一证据。[Navicat 功能矩阵](https://www.navicat.com/en/products/navicat-premium-feature-matrix)

旧的 `navicat-uncloned-audit.md` 和 `navicat-partial-rebuild-audit.md` 含实施前快照；本次以当前源码与 `sqlshop-rebuild-plan.md` 顶部 2026-10-19 的落地记录重新判断。多格式和自定义证书是该批明确未纳入的范围，本报告将其列为现有差距，不认定为上一批承诺未完成。

建议实施顺序：先解决写入截断和所有 SQL 文档的草稿丢失，再统一参数比较及任务连接初始化、修复格式化，并将复现加入持续回归。之后按实际使用场景选择多格式导入导出、证书与新增引擎工作区；大规模迁移、模型和后台调度分别立项。

## 本机证据

证据目录：`C:\Users\adm\.codex\visualizations\2026\10\23\01a166a2-a171-7452-9471-604d4822d61f\audit-evidence`。

- `data-correctness-repro.sql` / `.log`：PostgreSQL 数据正确性复现。
- `job-connection-repro.rs` / `.log`：MySQL 调度、PostgreSQL 初始化目标复现。
- `formatter-repro.rs` / `.log`：当前格式化函数复现。
- `navicat-win-17.2.7-rea-evidence.json`：REA 完整静态资源 Evidence。
- `navicat-native-analysis-attempt.json`：本轮 Ghidra 超时摘要及证据边界。
- 测试输出：`%TEMP%\sqlshop-audit-cargo-test-msvc2022-20261023.log`。
- 格式检查：`%TEMP%\sqlshop-audit-fmt-20261023.log`。
- Clippy 输出：`%TEMP%\sqlshop-audit-clippy-20261023.log`。

这些是当前机器的证据路径；没有把原应用、数据库凭据或大量第三方资源复制进仓库。

## 2026-10-23 修复与保存查询落地

上面的七项缺陷已修复，并补充回归：

- PostgreSQL 字符参数不再通过带长度 CAST 提前截短；嵌套 domain 仍由实际目标列执行长度和 CHECK 约束。JSON/XML 与 citext 的旧值核对采用精确读取表示，恢复核实区分旧值和目标值转换。
- SQL Server 文本 / 二进制保留完整输入，写前检查 UTF16 / 字节长度；varchar/char 按实际列 collation 做无损代码页往返检查。固定文本和二进制的恢复核实先验证长度，再处理填充。
- SQLite / Oracle 的 SQL、保存基线、文件路径和来源进入工作区保存与恢复；恢复不打开数据库、不连接、不执行，Oracle 密码不持久化。打开文件和生成对象 SQL 遇到旧草稿时先处理替换确认。
- 工作区格式升级为 v2，可读入并迁移 v1；旧客户端会拒绝新格式，避免把原生引擎草稿误解释为另一引擎的未绑定 SQL。
- 调度和恢复预检直接连接明确目标库；MySQL 调度仅校验 schema 与数据库一致，不调用不支持的 schema 切换。
- 格式化和压缩保留 PostgreSQL 字符串拼接所需换行，实库执行结果保持 `foobar`。

保存查询已形成可直接使用的文档流程：

1. SQL 编辑区“保存查询”或 Ctrl/⌘+S 将未绑定文件的查询命名保存，保留连接 / 数据库 / schema；再次保存更新原条目。
2. Ctrl/⌘+Shift+S 另存命名副本。侧栏、编辑区及菜单均提供“已保存查询”，支持打开、搜索、改名和删除。
3. 当前版本的同一查询复用标签并保留未保存编辑；旧版本标签保留原 SQL 和目标、显示冲突提示，重新打开会进入新版。过期标签不能覆盖新版本。
4. 命名查询关联随工作区恢复。已有 SQL 文件的 Ctrl/⌘+S 始终保存文件，查询库与文件保存基线独立。

实库联测另发现 Windows PostgreSQL 18 使用 TCP reset 结束取消控制连接，旧驱动误判取消失败并丢弃管理会话。本次驱动补丁只在完整请求 flush 后接受该关闭信号；原查询仍须返回 57014 才确认取消。新增默认 EOF / reset / 意外响应三项协议回归，实际分页与末页取消及会话复用均已验证。

CI 的 `test-all` 已接入表设计、对象设计、取消及完整输入测试；按各套件的真实测试名筛选，20 次命令选择 26 项专用数据库用例。查询会话旧测试已适配实际 schema 回写与进度事件，并保留错误目标拒绝、事务和并发断言。

验收结果：

- 默认测试：264 通过，0 失败，27 项需专用数据库的用例默认跳过。
- PostgreSQL 18.3 独立临时实例：9 项实库用例通过，覆盖 `input_integrity`、`table_edit`、`table_browser`、`table_designer` 三项、`object_designer`、`query_sessions`、`databases`；后两项含 TCP / SSH 路径。
- `cargo fmt --all --check`、`git diff --check`、`cargo clippy --locked --all-targets -- -D warnings` 通过。vendored PostgreSQL 驱动保留已有依赖警告，项目 Clippy 门禁通过。
- Windows `cargo build --locked` 通过，调试程序位于 `target/debug/sqlshop.exe`；已用隔离截图模式检查 1280×820 浅色与 900×600 深色查询界面，保存查询入口和侧栏在两种布局下均可见。截图模式未连接真实服务器，也未读取用户工作区。
- MySQL 调度 / PG 初始化及恢复预检：5 项默认协议 fixture 测试通过；本机未运行真实 MySQL、SQL Server 或 Oracle 服务验收，新增 SQL Server 字符集用例由专用实例 / CI 覆盖。

本轮验证日志使用 `%TEMP%\sqlshop-fix-*-20261023.log`，与前半部分的修复前审计日志分开保存。
