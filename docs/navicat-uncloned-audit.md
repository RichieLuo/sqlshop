# 原功能缺口：随包帮助与本轮实现边界审阅

## 2026-10-19 REA 复核：实施前核心缺口

后续实施：用户排除第5、6项后，其余五组已按本批范围落地，支持范围和联测记录见[授权实施计划](sqlshop-rebuild-plan.md#2026-10-19-授权实施计划)。以下保留实施前的审计快照，不代表实施后的缺口列表。

核对工作区提交 `c4ae776` 的源码及本机 Navicat Premium Lite 17.3.11。此次只做静态审计，没有启动原应用、连接数据库或验证真实读写；下列“缺失”指专用入口和实现范围，不把手写 SQL 算作设计器，也不把未联测算作未实现。

### 本轮证据与覆盖

- **R19**：REA `decode_interface_builder`，Evidence `ev_4f8e9e6903efeab0146b2087c211b3ed5047b021ba67fef64a4e69c2e931e4e4`。定向解析 9 个 NIB，2323 个图节点、1013 条边；所选归档的对象、连接和层级覆盖完整，`truncated=false`。这不代表整个应用覆盖完整。
- 原通用 Mach-O 被当前 Ghidra 后端拒绝；ARM64 方法搜索触发全量自动分析后主动终止，没有产生可用于结论的新反编译证据。随后在临时派生包中放入既有 ARM64 切片和原包逐字节复制的 9 个资源，仅用于 REA 静态归档解码，未运行派生包。
- ARM64 切片 SHA-256 为 `f200508a133605213e0b7bc0bec9dd670f48d495f79c5994a1da9ac2ef5e359d`。资源原始路径与 SHA-256 保存在 `/Users/richie/.local/share/rea/navicat-handoff/navicat-gap-audit-20261019-derivation.json`；完整 REA 记录保存在同目录 `navicat-gap-audit-20261019-evidence.json`。
- **H19**：重新读取原包中文帮助中的 `query_editor.html`、`data_rdbms.html`、`mysql_table.html`、`postgresql_table.html`、`con_ssl_settings.html`、`import_wizard.html`、`export_wizard.html`、`query_builder.html`。帮助描述与 R19 控件连接均不证明 Lite 运行时已启用全部选项；本次没有确认原程序内部保存算法。

### 高频缺口（建议实施顺序）

| 顺序 | 核心功能 | 当前实现与尚缺范围 | 证据与源码边界 |
| --- | --- | --- | --- |
| 1 | 完整字段、约束与索引设计 | 已有增列、有限改列及普通索引；缺已有字段改名、默认值编辑、通用类型变更、主键/唯一/CHECK/外键的完整新增修改删除，以及生成列等属性。外键页目前是元数据显示，不能称外键设计已完成。 | R19：MySQL `_defaultValueComboBox`、`_expressionComboTextField`、自增等控件；PG 默认值、identity、序列属性；外键延迟选项与 CHECK/索引表格。`src/db/structure_edit.rs:6` 的变更枚举只有 AddColumn/AlterColumn/DropColumn/CreateIndex/DropIndex；`:299` 限制改列范围；`src/ui/structure_ui.rs:660` 外键页只显示元数据。 |
| 2 | SQL 编辑器的日常编码能力 | 已有高亮、查找替换和关键词/表列前缀候选；缺 SQL 美化/简化、schema/别名上下文补全和代码折叠。前缀候选不等于语义补全。 | H19 `query_editor.html` 描述格式化、点号范围候选、代码折叠。`src/sql_editor.rs:630` 仅接收 prefix/columns/tables 并做 starts_with；本轮搜索 `src/` 未发现专用格式化实现。原算法和完整 Lite 启用范围未知。 |
| 3 | 数据网格列管理 | 已有拖宽；缺列显隐/列选择、冻结/解冻、单列及全列适宽和对应设置管理。对象浏览列表已有列显隐，不能用来代替数据网格的实现。 | R19 `TableViewViewController.nib` 存在 `_columnSelectionTableView`、`ibHideColumn:`、`ibFreezeColumn:`、`ibUnfreezeAllColumns:`、`ibSizeColumnToFit:`、`ibSizeAllColumnsToFit:`。对照 `src/ui.rs` 数据网格及 `src/ui/grid_browse_ui.rs`；现有 `hidden_columns` 位于 `src/ui/object_browser_ui.rs`。连接事实不证明这些动作运行时启用。 |
| 4 | Schema 与非表对象的创建、编辑、保存 | 已有对象目录、只读定义及转查询草稿；缺专用 schema 创建/删除/属性管理，以及视图、函数、过程、触发器等完整设计保存流程。 | R19 MySQL 视图模块有 definer、SQL SECURITY、算法与 CHECK OPTION 控件；过程等范围沿用此前帮助证据，未新增其代码调查。`src/ui/object_browser_host.rs:233` 明确只读；`src/db/object_catalog.rs` 读取定义；本轮 `src/` 未找到 CREATE/DROP SCHEMA 管理链。 |
| 5 | 多格式导入导出与可复用向导配置 | 已有 CSV 映射/预览/导入和 CSV 导出；缺 TXT/XML/JSON 等格式及 SQL 数据脚本导出、可复用的导入导出配置。查询结果 CSV 仍限已收集的前 1000 行，完整表 CSV 是另一条路径。 | H19 明确写出 Lite 可导入 TXT/CSV/XML/JSON，可导出 TXT/CSV/HTML/XML/SQL/JSON。`src/ui.rs:4605` 导入文件过滤仅 CSV；数据输出走 `export_csv`/`export_csv_rows`。这里只要求原帮助列出的文本格式，不把 Excel 等高版本格式列为 Lite 核心缺口。 |
| 6 | 连接证书配置 | 已有 TLS 开关及信任服务器证书选项；缺连接级自定义 CA、客户端证书与私钥配置。 | H19 `con_ssl_settings.html` 描述 CA/客户端证书等引擎相关选项；`src/model.rs:71` 和 `src/db/mod.rs:261` 等连接分支当前没有相应配置链。具体应按引擎分别实现，不能复制一套选项到所有数据库。 |
| 7 | 表加载的真正停止 | SQL 查询标签已有真实协议取消，不再属于“完全未复刻”；表分页读取/末页统计尚未接到取消通道。 | R19 表查看器有 `_stopButton` → `ibStop:` 连接；`src/worker.rs:425`、`:433` 的分页请求与 `:1085` 的查询取消路径分开。原表停止的内部协议及运行效果仍未知。 |

### 已有基础、仍不完整的扩展范围

- 比较同步：每侧完整范围上限 1000 行；同引擎 UPDATE+INSERT，保留目标独有行。缺大表/全库迁移、删除同步、跨引擎自动类型映射与复杂约束部署，见 `src/compare.rs:9`、`:434`、`:917`。
- 备份/调度/模型：已有官方工具备份恢复、应用开启时 SQL 计划与只读外键关系图；仍缺日志链/PITR、应用关闭后的系统调度、完整模型正反向工程。可视化查询创建器、模型和多项自动化在原帮助标为更高版本功能，不作为已确认 Lite 核心阻塞。
- 执行计划与权限：已有真实基础后端；仍缺完整计划图形、节点筛选/图像导出，以及高级账号属性、DENY/GRANT OPTION 和 DROP LOGIN。不能再笼统列成“执行计划/权限管理未实现”。
- SQLite/Oracle：已有独立连接/文件、SQL 与定义文档，尚未与主工作区完整网格、树及设计器统一；不再列成“引擎完全缺失”。

建议优先补字段/约束设计 → SQL 编辑器 → 网格列管理 → 对象设计 → 多格式导入导出。证书配置和表加载停止可按实际使用需求前移。每一项原行为只核对到静态证据；查询取消、断线恢复、权限变更、备份与同步等已实现路径仍需单独实库验收。

H19 新增帮助文件 SHA-256：`import_wizard.html` 为 `5fffc6c9c4a1612f77c29e70962bb10d363e071ddb995c371fbf7dbd2dd0137d`；`export_wizard.html` 为 `8e30786c5b1db3a053de9c04eabcc1e06d5dbc56a40112293f09a214d47a7eac`；`query_builder.html` 为 `9ca2329971d8837aa971b114b278cef6786e2b21d69d8a4f34fd22ee6c595849`。其他沿用下文指纹。

---

更新：2026-10-14。

本次直接读取 `/Applications/Navicat Premium Lite.app/Contents/Resources/zh-Hans.lproj/Navicat Help/` 的中文帮助，结合 SQLShop 源码静态审阅。没有登录原软件，没有重新运行 Ghidra，没有数据库测试或真实账号变更。既有 REA / bounded Ghidra 证据仍以 [二进制发现](navicat-binary-findings.md) 与 [交接文档](navicat-rebuild-handoff.md) 为准；本文件的帮助摘要不充当新的 REA evidence ID。

## 如何判定“复刻”

- **随包静态流程**：帮助页能够证明入口、选项和文档描述的流程。
- **Lite 能力未知**：帮助包共用内容，部分页面明确列出 Premium、企业版或标准版。文件存在不证明 Lite 已解锁该能力。
- **流程复刻**：项目沿用有证据的目标选择、预览、执行、结果与错误处理关系。
- **项目新增**：原帮助未描述、或项目采用不同实现与范围的功能。能运行不等于原功能完整复刻。

## 功能对照与范围

| 范围 | 原随包帮助确认的流程 | Lite 证据边界 | SQLShop 本轮实现与仍需覆盖的范围 |
| --- | --- | --- | --- |
| 执行计划 | `query_explain.html`：解释全文或选区，结果置于解释页；提供 JSON、文本、网格、统计；可视化有节点统计、查找、筛选和图像导出 | 可视化明确列 Premium、企业版与标准版；未确认 Lite 可用 | 已有独立执行计划页与真实服务器计划读取；PG / MySQL JSON 树、原始计划、SQL Server XML 原文。通用 JSON 树是项目呈现方式；原生图形、节点阈值与导出尚不能称已复刻 |
| 真正停止查询 | `query.html`：查询设计器工具栏提供停止；`query_results.html`：执行失败停止并显示错误 | 文档确认按钮存在，不能证明原内部取消协议或晚到取消处理 | 项目按数据库协议发送 PG CancelRequest、MySQL KILL QUERY 与 TDS ATTENTION；按执行身份区分，服务器响应确认后更新状态。具体传输、安全身份校验与无重试保护是项目实现选择，原内部细节未证明 |
| 事务 | `data_rdbms.html`：数据编辑器有开始、提交、回滚；设置支持打开编辑器时自动开始事务。`data_edit.html`：未启用事务的粘贴不能还原 | 文档描述数据编辑器，不能自动推广成原查询编辑器的所有状态行为 | 查询 Tab 已有原持久连接上的事务操作和状态核实。网格草稿批量提交是项目的暂存提交机制；数据编辑器长期事务、自动开始偏好与原流程仍应单独对照 |
| 用户、角色与权限 | 用户/角色设计器有常规属性、成员关系、对象权限；权限管理员从对象树选对象，再在用户与权限网格授予 | 同包包含不同数据库、不同年代的账号设计器；具体 Lite 与服务器版本组合未核对 | 已有真实账号/角色目录、直接授权与成员关系、角色创建删除、成员授予撤销、基础表权限授予撤销、内联 SQL 预览确认。补齐 PG LOGIN 用户与 MySQL User@Host 的创建、密码修改、启禁与删除；SQL Server 支持 SQL 登录创建、密码修改、启禁及数据库用户映射/删除。服务器登录删除、高级认证插件、完整账号属性、DENY、GRANT OPTION 与全部授权范围仍未覆盖，不能标为完整账号设计器 |
| 备份与还原 | `backup.html`：常规、对象选择、高级选项；全对象或固定对象集；备份注释、锁表/事务和文件名。`restore.html`：选备份、选目标/对象、预览配置后开始；可能删除并重建对象再插入记录 | 内置备份总览明确列 Premium、企业版与标准版；Lite 可用性未知 | 数据库官方工具备份/还原若接入，应作为项目支持范围列出；不能声称兼容原 Navicat 备份格式。CSV 导出、重建 DDL 和 SQL 草稿不能替代原生数据库备份 |
| SQL Server 备份与还原 | `backup_sqlserver.html`、`restore_sqlserver.html`：先生成 SQL 再开始；备份设备/服务器文件、数据库/文件组、完整/差异/日志；还原点、RECOVERY/NORECOVERY、REPLACE 等 | 帮助确认服务器备份流程，运行权限与 Lite 能力未确认 | 基础 BACKUP / RESTORE SQL 预览可复刻该流程；服务器路径必须区分本机路径。未覆盖的文件组、日志链、恢复点等不能以一个按钮笼统标为完整实现 |
| 自动运行与计划 | `automation_task.html`：选择工作、从可用列表移至已选择列表、拖动排序、保存/运行、日志。`automation_schedule.html`：保存后设置小时/分钟、日期/星期；逗号和范围表达式；计划前保存连接密码 | 自动运行总览明确列 Premium、企业版与标准版 | 应区分本机应用存活时的调度、应用关闭后的系统调度、顺序批处理和失败日志。单一 SQL 定时执行是基础项目功能；不是原全部工作类型、邮件附件和调度策略的完整复刻 |
| 数据比较与同步 | `data_sync_1..4.html`：源/目标及插入更新删除选项 → 表/键/字段映射 → 比较计数与逐行差异、勾选记录 → 部署脚本 → 执行日志 | 总览明确列 Premium、企业版与标准版 | 只读有上限的差异预览是项目范围；只有真实生成并选择部署脚本、核对快照、执行与记录失败，才能标同步部署已实现。分页导出/导入和仅显示差异不能代替该完整流程 |
| 结构比较与同步 | `structure_sync.html`、`structure_sync_3.html`：比较两端对象结构，选择差异，部署脚本可重排、复制或打开到查询页，再执行 | 总览明确列 Premium、企业版与标准版 | 文本 DDL 比较不能冒充跨引擎无损迁移；原比较和部署范围需要分别标记。带重建限制说明的 DDL 应保留其限制 |
| 数据与对象传输 | `data_transfer_1..3.html`：选源目标或文件及选项 → 选对象、字段/记录集与批次 → 摘要确认 → 进度及成功/失败；同连接拖表直接复制，跨连接进入传输流程 | 总览明确列 Premium、企业版与标准版；跨引擎转换另有版本范围 | CSV 迁移、单表复制、导出 SQL 各自是有限范围。跨引擎类型映射、对象依赖、索引/约束/触发器、事务和批次失败恢复不能自动视为已完成 |
| 模型与 ER | `model.html`：模型工作区、概念/逻辑/物理模型、逆向工程、正向工程、比较、字典；`model_reverse_eng.html`：从库结构创建模型或同步差异 | 模型总览明确列 Premium 与企业版；Lite 未确认 | 根据已读取外键画只读关系图是项目 ER 浏览功能；不是完整可编辑模型、反向/正向工程及 `.nmodel` 兼容 |
| SQLite | `con_gen_settings_sqlite.html`：现有文件/新建 SQLite 3 等；`sqlite_database.html`：main、附加/分离、查看 sqlite_master；`security_sqlite.html` 描述用户认证 | 同包包含 SQLite 2、HTTP 与认证扩展选项，不能按现代普通 SQLite 能力照搬 | 普通 SQLite 连接和文件浏览若扩展，应独立实现文件读写及 WAL/事务范围；原帮助中的用户认证、加密及 SQLite 2 需要特定扩展或旧引擎证据，不能仅加用户名密码框就宣布支持 |

## 新增诊断与权限页的具体事实

实现文件：[诊断后端](../src/db/diagnostics.rs)、[权限后端](../src/db/security.rs)、[独立后台](../src/advanced_worker.rs)、[文档状态](../src/advanced.rs)、[内联界面](../src/ui/advanced_ui.rs)。

1. 会话、锁与累计查询统计直接读取服务器可见目录。服务器权限、扩展缺失等错误保留；不伪造空成功结果。目录查询有显示上限，统计是瞬时或累计快照，并非完整慢日志。
2. PG 统计先定位实际安装的 `pg_stat_statements` schema，并识别执行时间字段；缺扩展即明确不可用。MySQL 读取 performance_schema / information_schema；SQL Server 读取 DMV。
3. 计划只接受经过现有语句边界词法校验和保守只读词检查的 SELECT / WITH。PG、MySQL 不启用 ANALYZE；SQL Server 的 SHOWPLAN ON、原查询、OFF 使用独立批次。后台为每个请求新建连接，请求完成、超时或关闭后丢弃；SHOWPLAN 连接不进入普通查询会话。
4. 帐号与角色列表不读取密码/hash 字段。PG 读取角色属性与 ACL；SQL Server 区分数据库用户、数据库角色与服务器登录；MySQL 使用精确 User@Host，读取 SHOW GRANTS 与角色关系。账号快照使用 PG OID、SQL Server 类型/目录 ID/创建时间，以及 MySQL 插件/密码变更时间核对身份与登录属性；MySQL 没有稳定账号 OID，不能保证检测同一秒内重建为相同属性的账号。
5. MySQL 无可靠“无成员角色/普通账号”标志。角色边源端可标识正在充当角色的账号；孤立角色仍显示为用户，不按“已锁定”猜角色。服务器源码 `role_tables.cc` 将 FROM 定位到角色、TO 定位到成员；本项目遵循该实际定义，未根据手册中相反的文字交换端点。
6. 变更只接受具名类型和固定权限枚举，标识符按引擎引用；预览 SQL 必须精确匹配，写入前检查编辑事务保护与对象存在性。角色创建没有密码；账号表单在页面内编辑，密码预览统一遮盖，SQL Server 服务器登录与当前数据库用户分别管理。
7. 直接 ACL、角色关系、所有者与内置规则都可能影响权限；页面显示直接授权与已公开关系，没有宣称完整计算有效权限。
8. 未收到权限变更确认时不会自动重放。已收到成功确认但随后目录刷新超时，应显示“变更完成、目录刷新失败”，不重新归类为未知提交。
9. 账号密码不实现 Debug 或序列化；待处理文档仅保存不含密码的操作状态，表单输入生成预览后清除。真正含密码的 SQL 仅在执行边界生成，其输出被丢弃；服务器错误可能回显密码，因此账号执行错误不保留原错误或部分结果，只返回固定说明与错误代码。驱动的语句/服务器消息日志也需同步关闭，不能仅遮盖 SQL 预览。
10. MySQL 密码修改保留原认证插件，仅支持已确认的内置存储密码插件；创建使用服务器默认插件。PG 拒绝会被服务器识别为预生成密码验证串的文本。SQL Server 密码修改仅支持 SQL 登录，按权限可填写旧密码；Windows/Entra 身份的密码由其认证提供方管理。基础流程不编辑密码策略、有效期、全部角色属性或高级插件。

## 静态审查关注点

本节记录审查发现与修复后的静态复核，不把源码审阅当已执行数据库测试。已读到 Begin 的本地拒绝标记、事务尾状态 15 秒等待、取消任务 Drop 时 abort、保留已完成输出以及 TDS ACK 前禁止 EOF 的保护。

| 关注点 | 要求 | 静态复核 |
| --- | --- | --- |
| Begin 在 Active / Failed / Unknown 时本地拒绝 | 保留原用户会话；不能把本地拒绝当网络错误而丢弃连接，导致已有事务回滚 | 已使用 EditRejected，worker 将其视为保留会话 |
| 事务操作后的状态读取 | 纳入有界等待；网络/协议错误不能被默认 Unknown 掩盖同时仍宣称连接可用 | 已加入 15 秒等待，失败关闭连接并 Unknown |
| 停止请求晚到 | 按执行 UUID 和 armed/active gate 拒绝旧请求；原查询连接重用前完成取消传输/ACK；超时 Drop 时关闭 gate 并终止剩余控制任务 | Guard 保存 AbortHandle；Drop 关闭 gate 并 abort，正常结束等待控制任务 |
| 查询已返回、取消控制失败 | 关闭不安全的连接时仍保留已完成输出与原错误前已完成结果，不丢弃用户已收到的数据 | CancelTransportFailure 与 retain_query_output 保留输出、原错误 downcast，强制不复用连接 |
| TDS 取消 ACK | 原响应需继续处理 ENVCHANGE 并读取 DONE_ATTN；不能在 attention_pending 时提前将消息 EOF 当完成 | is_eof 排除 attention_wait_ack；ACK 后 typed Cancelled 允许保留会话 |
| 角色授权与完整账号设计器 | 清单分别注明角色授权、基础账号管理与高级账号属性 | 基础创建/密码/登录状态与数据库用户映射已加入；完整属性、高级认证、全部权限范围和服务器登录删除仍未覆盖 |
| 密码与确认目标 | 真正密码 SQL 不进入预览、历史、响应或日志；确认时核对连接、数据库、Schema 与账号快照 | typed 账号操作与密码没有 Debug/serde；预览保存目标快照，提交逐项核对，原执行错误被固定说明替换；驱动日志保护由连接层同步提供 |

普通查询完成后的事务尾响应与数据库/schema观察各有15秒等待预算，已完成输出在尾响应前保存；取消控制通道另限30秒，仍按执行身份保护后续请求。硬超时丢弃连接不证明此前写入未提交，页面不得自动重跑。

## 服务器规范交叉核对

估算计划与 ANALYZE 的执行差别依据 [PostgreSQL EXPLAIN](https://www.postgresql.org/docs/current/sql-explain.html)、[MySQL EXPLAIN](https://dev.mysql.com/doc/refman/8.0/en/explain.html)。SQL Server SHOWPLAN 需独立批次，其行为依据 [SET SHOWPLAN_XML](https://learn.microsoft.com/en-us/sql/t-sql/statements/set-showplan-xml-transact-sql)。

角色与基础权限 SQL 依据 [PostgreSQL GRANT](https://www.postgresql.org/docs/current/sql-grant.html)、[MySQL Using Roles](https://dev.mysql.com/doc/refman/8.0/en/roles.html)、[SQL Server ALTER ROLE](https://learn.microsoft.com/en-us/sql/t-sql/statements/alter-role-transact-sql?view=sql-server-ver17)。MySQL 角色边方向通过 [MySQL 官方服务器源码](https://github.com/mysql/mysql-server/blob/8.0/sql/auth/role_tables.cc) 的 `populate_roles_caches` 核实。

基础账号 SQL 对照 [PostgreSQL ALTER ROLE](https://www.postgresql.org/docs/18/sql-alterrole.html)、[MySQL SET PASSWORD](https://dev.mysql.com/doc/refman/8.0/en/set-password.html)、[SQL Server ALTER LOGIN](https://learn.microsoft.com/en-us/sql/t-sql/statements/alter-login-transact-sql?view=sql-server-ver17)。MySQL 通过原认证插件处理新密码；SQL Server 自身密码修改可能要求旧密码，外部认证登录不支持 SQL 密码修改。

普通 SQLite 不实现 GRANT/REVOKE，不能从原帮助的扩展认证页面推导出普通 SQLite 用户管理；依据 [SQLite 未实现的 SQL 功能](https://www.sqlite.org/omitted.html)。SQLx 此处请求 PG 协议 3.0，4 字节取消密钥适用于该协议；不能宣称已实现 PG 3.2 的可变长密钥，版本差异见 [PostgreSQL 协议总览](https://www.postgresql.org/docs/current/protocol-overview.html)。

## 随包帮助文件指纹

以下为读取到的原始 HTML 字节 SHA-256；文件未修改。相对路径以本文件开头的 `Navicat Help/` 为根。

| 帮助文件 | SHA-256 |
| --- | --- |
| `query_explain.html` | `00d252d890d6f4d4f9e8d954bab0cd5a1bdc7cb4867dc1298bb0c9e9ddaa885a` |
| `query.html` | `9f075e0a232be5034f88e51f99055919ad6ba28828e585cb13fa911d0b360560` |
| `query_results.html` | `249a0023aac797ff521ea38be16b47bc830f041bdf8bf2f3ba9bbda1b6299f9e` |
| `data_rdbms.html` | `3e65556cfca1416c8aee0525501c7ed64f1dd503683f683ffae89bb2c2f3d8b0` |
| `data_edit.html` | `c0c862367d1c4eef44294ff0d204a3f3f5622e78d114cfbae5f6d749f71308e3` |
| `security_privilege_manager.html` | `c29b1c2c21067a26ee963818212e9242f14fec61242f9f904c2470ed82609a9b` |
| `security_mysql_user.html` | `eca9cc475496d4aec6d7daeab0cb98626a1cccef98558d813e2964f86a3a1a6b` |
| `security_postgresql_role.html` | `a59bc1223da0fdede3110981658bfc7ab35923f8342e6936e791fdec51cc356b` |
| `security_sqlserver_db_user.html` | `685ec1cb66b641ba0663b59d8b95ca028f49d1bf272a9ae4e99edcd790e7991c` |
| `backup_restore.html` | `a089716167d4438c886df5cff52e383c747a6e7425d075cb69aabb8d89630209` |
| `build_in_backup_restore.html` | `3bf531d43f02a0cbf371a93a5785c39fc01c2c8f8a7a0ac89cbacae31e3bb973` |
| `backup.html` | `a8a4c41be5239272b0843bbb467007dc0c09b4e8176cb357d1b610a890d3904e` |
| `restore.html` | `9deada0bc45da54d3c73e6ccc6826222e984ccd918f882cc18020d8205d3f644` |
| `backup_sqlserver.html` | `6c440a83e0a49bee855fb25df4593a03c9a6dfa76b94427c65bd54f4a4ef3b52` |
| `restore_sqlserver.html` | `cb55d1bea54734d22f3505a0a61aa5d77bd16819c06fea1db5e0bae6dce4ae11` |
| `automation.html` | `3ff08357e522ddfebf60e73f9ac336b840281e2569ac04e2698f1d8b252936ed` |
| `automation_task.html` | `25d890dbd00f9507b447734b2a71b174fb961dd9f68d89e06afc7e23d0f92d98` |
| `automation_schedule.html` | `c015368d1d0058bef6d372651d317cae8a36dffe63fd67e98d7a43d66b54efc1` |
| `data_sync.html` | `74d1f4cb83a0d2c21bea13c21cfe1b2c65f1ac65da32ca349cfd6cc36acd1d46` |
| `data_sync_1.html` | `5c5a2a7f9d89c3e2bf9d137c6248031b0e20b1313744b413c2b40690f777a48d` |
| `data_sync_2.html` | `dc9812437219597d6adf06b37a8fd7d503bc4ff5c5f1616377b96c2bea879b69` |
| `data_sync_3.html` | `097d88b5d4d23a1fe51f6eb43c67af947e0abd7c923c775b6144e936a1618282` |
| `data_sync_4.html` | `b4e7377ceff15fd1b7f5549082db32ac002a44de42ec501711d66023ff68f6dc` |
| `structure_sync.html` | `5c85b1bc249359c7bd7d1873651dae3c27571cdea4368844920505246c0f2f32` |
| `structure_sync_3.html` | `087cf6eb9e6e68b5aab4d659399504d113de7c490bed46b9e8ac61b43e513e20` |
| `data_transfer.html` | `cf2653902ae4f7bf148f2871f558914746d0a5f269cf7e040310bdb08e613f3c` |
| `data_transfer_1.html` | `a8bbccdf69ac50d7327ead72dcd9c6a1780f8a52530e5af6e9458f374607553d` |
| `data_transfer_2.html` | `6a74e55d051cfb3139c24ff664c6651acc74b870947ac2c4bca5310827719dd8` |
| `data_transfer_3.html` | `c436c220de9da86ac28265d304e53b10546083f7a7272c2b75c7f48fa6a1e2c6` |
| `model.html` | `cb0bdb87657a2d256cb8440f01c42d8e47a8791c578804f88de2d90181582efa` |
| `model_reverse_eng.html` | `da92b9f44e96ab79fc0e2c81fda35a0e02d8643e534a2a0a09a8653c04d1ec40` |
| `con_gen_settings_sqlite.html` | `cf5215ce769833db2d51996dad600260735289b3ebbd488783b80c70bc99d79c` |
| `sqlite_database.html` | `6ece86997f68d27a3be7ea3812fef3fc63b3750c76fe5c1c1b9269a3ddfc52e9` |
| `security_sqlite.html` | `a644bd7d999db98998f71d460d89b3f52ef5b902360e0f26ef59e211910d4a05` |
