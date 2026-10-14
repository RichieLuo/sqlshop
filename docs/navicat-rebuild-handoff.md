# SQLShop 参考 Navicat 重建交接文档

## 最新补充：原“未复刻”功能接入（2026-10-14）

主表12、13、25、30–34已接通明确的基础范围；状态与剩余差异见 [功能清单](sqlshop-rebuild-plan.md)，帮助指纹/版本边界见 [本轮审计](navicat-uncloned-audit.md)。即时取消、事务、恢复均有真实后端；工具菜单新增计划、诊断、账号授权、备份、比较同步、调度、SQLite与Oracle，设计页可查看实际外键关系图。基础操作使用独立tab或页内区，不新增编辑弹窗。

新增 `query_control`、`recovery`及独立worker、`diagnostics/security/advanced`、`backup/jobs`、`compare`、`local_database/oracle_database` 与相应UI。MySQL Session Track/TDS ENVCHANGE保留ROW_COUNT/@@ROWCOUNT，SQLServer schema仅连接/主动检查读取；PGschema继续回写。取消按本次UUID隔离，控制通道结束后才复用连接，ACK前不宣称停止；不自动重连/重放写入。恢复的当前值不能证明历史COMMIT，未知INSERT继续保护。

保留完整原三引擎工作；新增引擎为独立文档，Oracle需要Instant Client。基本账号管理已接通，高级认证插件/完整属性/服务器权限、大表与跨引擎自动转换、系统后台调度、完整模型仍在表中明确列缺口。主表的“已复刻基础流程”不表示Lite已启用全部共用帮助内容或完整选项一致。

本轮只做源码/编译/Clippy/构建及离线原生布局检查，不新增或运行测试、不访问真实数据库，不执行备份/迁移/账号变更。普通用户窗口未强制关闭或重启；启动仍为 `cargo run --locked`。测试授权需以用户后续明确请求为准，历史联测不能当成本批验收。

最终检查：fmt、全目标check、全目标Clippy（`-D warnings`）、build（均`--locked`适用）及差异格式通过；原生离线宽浅色/窄深色工具页面截图在 `/tmp/sqlshop-tools-20261014`。上游PG驱动四条警告保留，主项目无Clippy错误。

## 最新补充：对象页内容扩展（2026-10-14）

对象页已从表 / 视图三列列表扩展为真实目录工作区：详细信息 / 图标、分类计数、属性搜索、列头排序 / 列显隐 / 横滚；按服务器可用信息显示注释、引擎 / 所有者、估计行数、目录大小、时间和签名 / 所属对象。函数 / 过程 / 触发器 / 适用序列及PG物化视图已接入。树当前仍以表 / 视图为主，扩展对象从中央类别浏览。

新增 `src/object_catalog.rs`、`src/db/object_catalog.rs`；选库及刷新对象页读取目录，未知属性留空，目录数量达到每组1000项上限或缺权限有提示。大小 / 行数是目录指标，按引擎解释，不扫数据表。函数等对象打开独立只读定义tab，按连接 / 库 / schema / 类型 / 名称 / 重载签名 / 所属表复用；按源UUID接收结果，可复制 / 刷新 / 转查询草稿。定义页不执行SQL、不保存为查询草稿；上下文变化标旧定义待刷新，定义不可见等本地拒绝保留管理会话，传输错误断线。

主清单第3、4、29项已更新；静态 / 随包帮助依据见 [对象工作区审计](navicat-object-workspace-audit.md)。原生离线模式新增 `objects-rich`、`objects-icons`、`object-definition`，仅示例、不联网、不读写用户配置。真实数据库验收、完整对象设计管理、更多类别及原应用默认列 / 像素仍待做。

## 最新进展：优先收尾“部分复刻”（2026-10-14）

已按主表九项补核心交互；最新清单与剩余范围见 [sqlshop-rebuild-plan.md](sqlshop-rebuild-plan.md)，本批参考审计见 [navicat-partial-rebuild-audit.md](navicat-partial-rebuild-audit.md)。没有直接把状态改为“完全复刻”。

- 已有记录网格内输入、页内展开长文本，Tab / Enter连续编辑、Esc取消；范围NULL / 空串 / 撤销和TSV粘贴先全块检查再暂存。提交同步正在输入的格；所有文档导航与关闭保护输入，手动事务提交策略保留。
- PG schema目标保存 / 继承 / 下拉 / 异步独立目录读取（不挡用户SQL） / search_path / 实际schema回写闭环；SQLServer读取 / 显示 / 回写实际用户默认schema、恢复时核对，原程序selector用途仍须查证。MySQL使用库上下文。
- 多结果标签、消息页，批处理后续失败保留完整前序结果与已收消息；结构化错误定位，选区映射且原文变化后禁定位。三引擎原批次空结果保留列；原始PG NOTICE、SQLServer PRINT / INFO / DONE行数、MySQL警告计数接到消息页，NOCOUNT不伪造0，DONEPROC汇总不重复生成结果，服务器消息显示上限512条。驱动新路径未做真实数据库验收。
- 对象树折叠 / 已连接过滤 / 搜索，四页信息栏（常规 / 字段 / DDL / 查询预览）、SQL文件身份复用、拖排 / 快捷切换 / 焦点恢复、设计元数据查找。
- 页 / 记录首前后末与跳页，末页复用参数化筛选COUNT / COUNT_BIG；失败不改偏移。View布局开关和专注模式，主题 / 布局持久化，窄窗手动打开信息栏生效；修正侧栏搜索边距撑宽与PG三个目标控件预算，底条记录导航预留两行。

驱动收尾新增 `vendor/sqlx-mysql`、`vendor/sqlx-postgres`、`vendor/tiberius` 原版本本地补丁，Cargo patch及lock已接入，原许可证与来源说明保留。升级时须复核移植补丁，直接移除patch会回退批处理空列 / 服务端消息。主表第6、20、24项已更新；SQLServer参考selector用途与运行时像素继续标待核对。

新增源码：`src/db/query_context.rs`、`src/ui/query_result_ui.rs`。运行仍是项目目录 `cargo run --locked` 或 `target/debug/sqlshop`；新功能需要新构建进程。原生示例截图模式新增 `query-schema` / `query-messages` / `query-error` / `query-sqlserver` / `focus`，不读取用户Store、不联网、不保存工作区。普通`cell-edit`已变为网格内编辑，`cell-edit-expanded`展示页内长文本编辑。

本轮格式、全目标check / Clippy、构建、差异格式与原生示例截图检查通过；未新增或运行测试、未执行真实数据库查询 / DDL。全目标检查只编译现有测试源码。截图先前在 `/tmp/sqlshop-partial-20261014`，驱动收尾消息页与SQLServer上下文在 `/tmp/sqlshop-finish-20261014`；前文历史测试不能作为本轮验证。原运行进程保留，重启后载入新构建。


更新日期：2026 年 10 月 14 日，Asia/Shanghai。

任务是在现有 SQLShop 上参考 Navicat 的界面与操作流程，重建日常数据库管理体验。REA 环境已安装并完成静态分析和原生反编译验证。接手智能体应先建立可验证的功能规格，再分阶段实现；保留已有数据库、SSH、存储和测试能力，并继续使用 SQLShop 的品牌与图标。

## 接续进展与最新用户要求（2026-10-14）

用户要求按重要性列功能表，并标记项目是否实现 / 复刻。统一清单位于 `sqlshop-rebuild-plan.md` 的“功能清单（按重要性排序）”：35 项、P0–P3，最后一列分别记录实现与复刻状态；不能把功能存在直接算成完全复刻。后续每批同时更新该表的范围、缺口和状态。本次仅核对源码与已有证据、更新文档，未运行测试。

用户已明确要求直接通过二进制反编译和静态资源复刻；Navicat 登录、原应用运行观察及辅助功能权限不作为实施前置。SQLShop 已有功能如果质量不如参考，可以重写覆盖；保留工作区不是冻结旧界面或流程。

- 已完成规格和计划：`navicat-reference-spec.md`、`sqlshop-rebuild-plan.md`；二进制调查记录为 `navicat-binary-findings.md`。
- Clippy 的 bitmap 初始化问题已修复；新增暂存值网格显示、提交/筛选保护、过期快照保护、提交成功后刷新失败提示，以及当前 SQL 语句执行。109 项默认测试与三引擎九项联测均通过，格式/Clippy/差异检查通过；已检查三张 SQLShop 原生窗口截图。
- 已解析 9 个主窗口/对象树/查询/表格相关 NIB，全部无截断，并保留每个原始资源的校验映射。静态提交、撤销、刷新、NULL/空串入口已找到。
- 原始通用 Mach-O 可用 `open_binary` 的 `provider_id: "auto"` 绑定做原生静态工具操作；显式 Ghidra 会拒绝通用 Mach-O。深度反编译已选择已有 ARM64 副本，不修改原应用。
- 从 `otool` 与 `nm` 相互核对找到表格核心方法真实 IMP，索引为本机 `navicat-table-actions-symbols.json`。正在沿真实函数和下游数据模型分析，而不是依据菜单名推定事务行为。
- 后续已完成直接 NULL/空串、单格撤销、F2/Enter 编辑和固定表底操作条重构，草稿详情默认折叠；最新默认测试 116 项通过，并检查 900×600 浅色与 1280×820 深色新布局。
- 主程序全量 Switch 扫描已主动结束，改为按真实函数边界做有界 Ghidra 反编译。核心库保存/事务 6 个函数、主程序表格相关 12 个函数已成功生成伪代码，分别保存在本机 `libcc-bounded-ghidra` 与 `navicat-main-bounded-ghidra`。这些独立 Ghidra 输出不能冒充 REA Evidence。完整命令、SHA、范围、原始和注解输出、局限均在本机记录；底层执行器与完整事务范围仍待继续分析。
- 用户使用后指出尚未复刻核心体验。本轮再有界反编译 20 个导航 / 对象 / 文档 / 查询入口与 8 个支撑函数，28/28 成功，记录为 `navicat-workflow-bounded-ghidra`；N7 核对浏览模型、文档身份复用、新查询上下文、目标控件到后台执行的实际链。每文档 holder/server 不能据此宣称独立网络 socket。
- 已重做对象工作区：左树连接 / 库 / schema 选择，中间表与视图列表、搜索和分类，右对象信息；打开对象复用其身份标签，空 / 过期快照沿原保护刷新，浏览时后台文档不显示选中。宽度不足 1100 自动收起右栏。
- 已实现并接入每 SQL 标签的持久 `QueryWorker`，与表 / 目录管理会话隔离。同连接查询可并行，临时表 / 变量 / 显式事务跨执行保留；管理切库不改变查询，USE 仅跟随源标签，普通 SQL 错误仍可 ROLLBACK，断线不自动重跑。已有会话改绑、关闭、重连和删除有清理与确认；忙标签禁止关闭或改绑。新查询从当前对象工作区或活动文档继承目标。
- 本轮查询后端三引擎 TCP+SSH 六路径与传输掉线协议 fixture 已通过；最终 `cargo check --all-targets`、格式、全目标 Clippy、构建与差异格式检查通过。原生示例截图位于 `/tmp/sqlshop-core-20261014`，包括对象区 1280×820 浅色 / 900×600 深色、查询 900×600 浅色、表暂存 900×600 浅色。未重新运行整套默认 UI 测试，前述 116 项为上轮结果；示例截图不验证真实数据库 GUI 操作。
- 本段为早期查询会话重构记录；即时停止、事务及schema最新实现见本文顶部。整段SQL总超时3600秒，事务尾响应/上下文观察另限15秒；关闭Worker仍不等于服务器确认取消。既有专用测试套件保留，本轮未运行。
- 最新调试构建已按普通模式启动，读取用户现有配置 / 草稿，不自动连接数据库；已确认进程运行、一个可见原生窗口并激活。当前可执行文件为 `/Volumes/0/WorkSpace/sqlshop/target/debug/sqlshop`，未安装替换发行包，工作区改动仍未提交。
- 用户继续指出对象区和 tab 栏的差距。本轮用 `ui/tab_bar_ui.rs` 统一固定对象页与文档标签：按标题宽度收缩、类型图标、顶部选中标记、悬停 / 活动时显示关闭、全部标签菜单和信息面板图标。同名对象用连接名区分，菜单 / 提示显示连接与库，保留原关闭保护。对象分类改为图标与计数页签，搜索在右侧，操作条在分类下方，列表改为 25px 行高和轻选中态。此视觉几何为 SQLShop 的实现选择，N4 未提供运行时标签控件的精确外观；不宣称像素级复刻。标签栏样式限定在局部 scope，避免挤压查询工具栏。原生示例截图保存于 `/tmp/sqlshop-tabbar-20261014`；新版构建未强制关闭此前仍运行的窗口，该窗口重新打开后才会使用更新后的可执行文件。

- 对象 / tab 栏迭代收尾时原进程已退出，随后普通启动了更新后的调试构建，确认一个可见窗口并激活；格式、全目标 Clippy、构建、差异检查及宽浅色 / 窄深色原生示例布局检查通过。本批未新增或运行测试。后续按用户实际使用反馈继续校准对象与文档的层级、样式和操作。

下面环境与初始检查记录保留为历史交接，后续按上述最新要求继续。

### 用户截图后的元素位置修正

用户肯定本版样式与颜色，但指出元素位置仍有问题。根据其未连接对象页与查询页截图，取消中央面板外边距，让 tab 栏贴齐工作区分隔线与顶部；12px 水平 / 8px 垂直边距移到内容区。查询页改为首行连接 / 数据库目标，分隔线后集中运行、文件、查找和历史操作；目标下拉宽度按可用空间调整，会话状态缩短，重建入口归入会话菜单。配色和标签样式保持本版设计。新增 `browser-offline` / `query-offline` 示例截图模式，均不读取用户配置或访问数据库；截图保存于 `/tmp/sqlshop-alignment-20261014`。新版构建 / Clippy / 格式与差异检查通过，原有运行窗口未强制关闭。

用户再次指出 tab 内仍有偏差。原生布局坐标确认：对象 tab 为 y=124..156，文档 tab 为 y=128..160，整个文档标签下移 4px。根因是 32px 滚动视口内嵌默认 24px 高的居中 horizontal 行。已把该行改为从视口顶部直接布局，复查两者均为 y=124..156，图标 / 文本 / 选中线同步对齐；临时测量输出已移除。最终浅色 / 深色截图分别为 `/tmp/sqlshop-alignment-20261014/tabs-fixed-light.png` 和 `/tmp/sqlshop-alignment-20261014/tabs-fixed-dark.png`，均已检查。之前外边距修正没有覆盖此内部偏移，不能继续用旧截图宣称对齐完成。

### 数据网格操作入口

用户肯定对齐修正并要求继续找可复刻的核心。本批补 `ui/grid_browse_ui.rs`：列头升序 / 降序 / 清除、Shift 追加多列排序、右键指定方向与列筛选；单元格右键将原始快照值、NULL 或空字符串带入筛选草稿，文本支持包含。已有应用条件保留，新增条件待检查后点击应用；工具条区分待应用和已应用，表头显示排序优先级与筛选标记。服务端排序重新读取第一页，不重排本地行与原始主键。所有入口沿用忙碌、编辑窗、暂存、快照和目标保护，普通 SQL 结果不启用表操作。

现有 N4/N6 证据尚未确认精确列头 / 单格筛选行为，本批属于 SQLShop 操作改进；下一块仍优先做正式表结构设计文档，再补连续网格编辑与查询 schema / 取消。格式、全目标 Clippy、构建与差异检查通过；原生示例截图在 `/tmp/sqlshop-grid-20261014`，未新增或运行测试，未操作真实数据库。运行中的旧窗口保留，重新打开应用后使用本批构建。

### 用户要求网格内新增行

用户反馈“＋没有用”后已澄清：按钮可以打开新增弹窗，但希望在表格下面直接新增填写。本批移除新增行弹窗，新增 `ui/table_insert_ui.rs`：＋追加独立草稿行并滚动 / 聚焦，直接输入、多行草稿、默认 / NULL / 空串、−或右键移除草稿，提交全部 / 撤销暂存。只含自增 / 生成列的普通表也允许默认新增；草稿不混入读取快照、原始主键或 CSV。所有草稿保护已统一包括新增行，CSV 导入窗口也加入工作区 modal 保护。

新增 `Session::save_table_draft`、`Request::SaveTableDraft` / `Response::TableDraftSaved`，原行乐观 UPDATE 与多行 INSERT 同一事务；普通错误回滚成功后保留连接 / 草稿，网络、回滚或提交不确定仍断线并标过期。成功只清来源 tab 草稿并刷新，明确提示新记录可能受条件 / 排序 / 分页影响。既有 CSV 导入路径保留。源码核对、格式、全目标 Clippy、构建和差异检查通过；原生示例截图在 `/tmp/sqlshop-inline-20261014`（宽浅色混合草稿、窄深色空表新增），未新增或运行测试、未写真实数据库。旧用户进程保留，重开应用加载新构建。功能清单第 10 / 18 项已更新；正式结构设计 tab 仍待做。

### 表结构设计改为工作区文档

用户进一步明确：结构设计也不应使用全局弹窗，基础编辑交互应采用参考的工作区方式。本批新增 `ui/structure_ui.rs`，移除 `App.structure` / 全局结构草稿 / 全局 SQL 确认窗口。每个设计 tab 具有 `DesignDocument`：结构元数据、字段 / 索引 / 外键 / DDL 子页、选择、输入、页内 SQL 预览确认、错误与过期状态。按连接 / 库 / schema / 对象 + 数据或设计角色复用文档，不覆盖数据 tab 或其他表设计输入。数据页“结构 / DDL”、对象列表 / 信息面板的结构入口均打开设计 tab，设计页可回到数据文档。

有限修改能力保留：新增可空无默认列、PG 依赖检查后删除列、普通单列索引创建 / 删除；完整改列、默认值与约束编辑仍待做。新布局没有扩大后端 DDL 能力。刷新保留输入、清理预览；失败保留输入并标结构待刷新；修改成功清源设计输入并使同对象数据页过期。结果按来源 UUID 处理；设计输入参与未保存标记与关闭 / 退出确认，不持久保存到 workspace.json。右信息栏使用当前设计文档元数据，不误取其他数据 tab。N7 的文档角色是静态依据，精确布局与页内确认以用户要求 / 项目设计标明。

格式、全目标 Clippy、构建与差异检查通过；原生字段 / 编辑 / 索引 / 确认示例截图位于 `/tmp/sqlshop-design-20261014`，均不读取用户配置或访问数据库。未新增或运行测试，未执行真实数据库操作。功能清单第 14 项已更新，后续优先第 15 / 18 项。

结构设计类型选择补齐：用户要求字段类型用下拉框，现新增字段表单按文档绑定引擎提供候选，复用既有类型列表并用现有 ADD COLUMN 生成器做纯本地筛选，不展示后端尚不支持的类型。VARCHAR / CHAR / 二进制等需要长度的类型显示必填长度；切换为无长度类型时清除旧长度。没有扩大已有字段类型修改或约束能力；功能清单第 15 项已更新。编译 / Clippy / 格式 / 差异与原生示例布局检查通过，未新增或运行测试、未访问数据库。

### 已有字段编辑

已有字段编辑已补：选中字段点“编辑字段”，或双击 / F2，页内载入原类型 / 长度 / 可空设置，按引擎下拉并保留原字段完整元数据行。未改值不标未保存，也不能发出无修改预览。新增 `StructureChange::AlterColumn` 和 `ColumnEditInfo`，支持同类型长度扩大 / 整数扩大 / 可空设置，禁止缩短与任意隐式转换；执行前重读原字段行并检查索引 / 约束 / 外键 / 表达式等依赖。MySQL 要求 InnoDB / 严格模式并重述原 charset / collation，带默认 / 注释 / extra 等复杂属性暂拒绝；PG 单条 ALTER 组合且保留 collation，SQL Server 保留 collation并限制默认约束 / 生成属性。无改名 / 默认值编辑。本批不宣称 MySQL 并发 DDL 校验与执行原子隔离，也未恢复原程序完整改列策略。

格式、全目标 Clippy、构建、差异与宽浅色编辑 / 窄深色 SQL 预览示例检查通过；截图 `/tmp/sqlshop-design-20261014/existing-field-edit-light.png`、`existing-field-preview-dark.png`。未新增或运行测试、未执行真实 DDL。功能清单第 15 项同步更新。

## 接手后的第一步

1. 完全退出并重新启动 Codex，让新会话加载 REA MCP 和匹配的 skill。
2. 打开 `/Volumes/0/WorkSpace/sqlshop`，阅读本文件和项目 `README.md`，检查 `git status --short`。当前有用户原有改动及本轮修复，均未提交，不要重置或覆盖。
3. 检查当前会话是否提供 REA 工具。配置健康不等于当前会话已经连接；工具缺失时先重连或查看启动错误，不要重复安装。
4. 读取已有证据，先分析“打开表 → 编辑多格 → 提交或撤销 → 刷新”这一完整流程，产出规格和实施计划，然后再改 SQLShop。

## 已安装的环境

| 项目 | 版本或位置 |
| --- | --- |
| REA 运行包 | `rea-agents@6.4.0`，安装于 `/Users/richie/.local/lib/node_modules/rea-agents` |
| 普通 REA CLI | `/Users/richie/.local/bin/rea` |
| 本任务 CLI 启动器 | `/Users/richie/.local/bin/sqlshop-rea`，固定 Node、JDK、Ghidra 和分析超时 |
| Node.js | 现有 `/usr/local/bin/node`，24.14.0 |
| JDK | 现有 OpenJDK 24.0.2，ARM64 完整 JDK |
| JAVA_HOME | `/Users/richie/Library/Java/JavaVirtualMachines/openjdk-24.0.2+12-54/Contents/Home` |
| Ghidra | 12.1.4，`/Users/richie/.local/share/rea/tools/ghidra_12.1.4_PUBLIC` |
| 原生反编译器 | `Ghidra/Features/Decompiler/build/os/mac_arm_64/decompile`，已通过随包 Gradle wrapper 编译 |
| Codex MCP 配置 | `/Users/richie/.codex/config.toml` 中的 `mcp_servers.rea` |
| REA skill | `/Users/richie/.agents/skills/reverse-engineer-anything/SKILL.md`，发布包匹配版本 37 |
| REA 源码参考 | `/Volumes/0/WorkSpace/rea`，保持未修改；不是当前 CLI 的安装目录 |
| 日志与证据目录 | `/Users/richie/.local/share/rea/navicat-handoff` |

安装使用发布包，不要求把 REA 源码仓库再编译一遍。源码 main 中的 skill 为版本 42；接手时优先遵循已安装版本 37 和当前服务实际公布的工具 schema，避免使用尚未进入安装包的参数。

Ghidra 发布归档的 SHA-256 已与[官方发布元数据](https://github.com/NationalSecurityAgency/ghidra/releases/tag/Ghidra_12.1.4_build)核对：

```text
ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db
```

归档通过镜像分段下载后进行完整校验，安装来源、字节数和校验值保存在 `ghidra-installation.json`。该版本的官方归档不含 macOS 原生组件，已按随包 `GettingStarted.md` 的 `buildNatives` 流程补齐；日志为 `ghidra-build-native.log`。没有安装 Hopper 或 IDA，没有更换现有 Node.js/JDK。

### Codex 启动配置

REA 使用绝对 Node 路径启动，避免依赖 GUI 进程的 PATH。仅修改了 REA 服务配置，保留其他配置和已有工具审批策略。原配置备份为：

- `/Users/richie/.codex/config.toml.rea.backup`：REA setup 写入前。
- `/Users/richie/.codex/config.toml.rea-runtime.backup`：固定启动器与超时前。

关键配置如下；已写入，不需要再次粘贴：

```toml
[mcp_servers.rea]
command = "/usr/local/bin/node"
args = ["/Users/richie/.local/lib/node_modules/rea-agents/scripts/rea.mjs", "mcp"]
startup_timeout_sec = 30
tool_timeout_sec = 1200

[mcp_servers.rea.env]
GHIDRA_INSTALL_DIR = "/Users/richie/.local/share/rea/tools/ghidra_12.1.4_PUBLIC"
JAVA_HOME = "/Users/richie/Library/Java/JavaVirtualMachines/openjdk-24.0.2+12-54/Contents/Home"
REA_ANALYSIS_PROVIDER = "ghidra"
REA_GHIDRA_STARTUP_TIMEOUT_MS = "900000"
```

首次 Ghidra 导入和自动分析可能较慢，服务侧启动限时为 15 分钟，Codex 单次工具限时为 20 分钟。`startup_timeout_sec` 和 `tool_timeout_sec` 控制不同阶段，配置含义见 [OpenAI 配置参考](https://learn.chatgpt.com/docs/config-file/config-reference)。客户端超时后不要立即启动重复分析，先检查原会话和提供方状态。

### 可直接执行的环境检查

```sh
/Users/richie/.local/bin/sqlshop-rea --version
/Users/richie/.local/bin/sqlshop-rea doctor --client codex --provider ghidra --skill --json
/Users/richie/.local/bin/sqlshop-rea mcp doctor --json
```

本轮验证结果：

| 验证 | 结果 |
| --- | --- |
| Codex 注册、Ghidra 及匹配 skill | `doctor-final.json` 中 `healthy: true`，所选检查全部通过 |
| 实际 MCP stdio 启动、工具目录和基本请求 | `mcp-smoke.json` 中 `healthy: true`，170 个工具、6 个 prompts 与预期一致 |
| Ghidra 原生反编译 | 对自建 ARM64 Mach-O 测试函数 `_rea_handoff_add` 成功生成伪代码与汇编；`ghidra-smoke.json` |
| Navicat Mach-O 静态检查 | 成功，`navicat-macho.json` |
| Navicat 签名检查 | 成功，`navicat-signature.json` |
| Navicat Interface Builder 资源解析 | 成功解析前 64 个文档；有截断，不能当作完整应用规格 |
| Navicat 窗口枚举脚本 | 已验证可运行；检查时 Navicat 未运行 |
| Navicat 截图、控件树和点击场景 | 尚未验证，需要运行中的应用及 macOS 权限 |
| Navicat 全量 Ghidra 分析 | 尚未运行；原生引擎验证使用独立小型测试工件 |

170 个工具的目录检查不代表全部能力均可在当前机器使用。缺少其他可选引擎不影响已验证的 Ghidra 和静态原生分析路线。

## 分析目标与已有证据

本机目标为 **Navicat Premium Lite 17.3.11**，bundle ID 为 `com.navicat.NavicatPremiumLite`。功能边界以本机 Lite 版实际可用行为为准。

| 对象 | 绝对路径 |
| --- | --- |
| Navicat 应用包 | `/Applications/Navicat Premium Lite.app` |
| 原始可执行文件 | `/Applications/Navicat Premium Lite.app/Contents/MacOS/Navicat Premium Lite` |
| 已准备的 ARM64 静态分析副本 | `/Users/richie/.local/share/rea/navicat-handoff/navicat-arm64` |
| 副本来源与校验记录 | `/Users/richie/.local/share/rea/navicat-handoff/navicat-arm64-derivation.json` |

原程序为包含 x86_64 和 arm64 的通用 Mach-O。ARM64 副本由 `lipo -thin arm64` 导出，原应用未修改。深度静态分析可使用副本，界面观察应绑定原始 `.app` 或实际运行的原始可执行文件。两者的路径与 SHA-256 不同，证据引用时必须保留来源关系；不要将副本地址、原文件偏移和另一架构的地址混用。

原始可执行文件 SHA-256：

```text
47b428be0693e9cb353ea1ba1ef056d6075b1e2fcd944b60d632da63f342a1f6
```

ARM64 副本 SHA-256：

```text
f200508a133605213e0b7bc0bec9dd670f48d495f79c5994a1da9ac2ef5e359d
```

### 复用这些文件

证据均在 `/Users/richie/.local/share/rea/navicat-handoff`，无需重复跑已有检查：

| 文件 | 内容与限制 |
| --- | --- |
| `navicat-macho.json` | 原始二进制的架构、依赖、导入导出、段和构建信息 |
| `navicat-signature.json` | 签名、权限与 hardened runtime 信息 |
| `navicat-interface-builder.json` | 64 个界面资源文档，5570 个图节点、2293 条边；`graph.truncated: true`，覆盖范围有限 |
| `navicat-arm64-derivation.json` | 原件、副本、架构与校验值的对应关系 |
| `ghidra-smoke.json` | 可复用的引擎验证证据 |
| `doctor-final.json`、`mcp-smoke.json` | 环境与服务检查记录 |
| `setup-preview.json`、`setup-result.json`、`codex-runtime.json` | 安装计划、执行结果及最终 REA 配置 |
| `list-navicat-windows.swift` | 仅枚举指定 bundle ID 且可执行路径匹配的 Navicat 窗口 |

界面资源和类名只提供候选入口，不能证明功能在 Lite 版已启用，也不能证明点击后的运行行为。需要更多资源时先查看安装版 `decode-interface-builder --help`，按问题扩大范围，并检查 `coverage`、`truncated` 和 `limitations`。

### 静态分析命令

CLI 使用同一任务启动器。将完整 JSON 保存到文件，再读取必要字段，避免将大量伪代码或资源图直接塞进上下文。

```sh
REA_CMD=/Users/richie/.local/bin/sqlshop-rea
NAVICAT_APP='/Applications/Navicat Premium Lite.app'
NAVICAT_ARM64=/Users/richie/.local/share/rea/navicat-handoff/navicat-arm64

# 以下三项已有结果，按需复用已有文件。
"$REA_CMD" inspect-macho "$NAVICAT_APP" --json
"$REA_CMD" inspect-signature "$NAVICAT_APP" --json
"$REA_CMD" decode-interface-builder "$NAVICAT_APP" --json

# 深度分析：先选定一个具体功能，再执行必要的查询。
"$REA_CMD" inspect "$NAVICAT_ARM64" --provider ghidra --json
"$REA_CMD" search "$NAVICAT_ARM64" 'commit' --kind procedures --provider ghidra --json

# 从返回的清单选择真实存在的符号或地址，不凭印象写名称。
# "$REA_CMD" function "$NAVICAT_ARM64" --procedure '实际返回的符号' --provider ghidra --json
```

每次 CLI 调用有独立会话。持续调查优先使用 MCP，在一个 `open_binary` 会话中复用分析结果，结束时调用 `close_binary`。MCP 打开 ARM64 副本的输入为：

```json
{
  "path": "/Users/richie/.local/share/rea/navicat-handoff/navicat-arm64",
  "provider_id": "ghidra"
}
```

检查调用结果的证据、截断与未知项。伪代码用于理解候选实现，不保证还原原始源码，也不会自动转成 Rust 工程。

### 界面观察流程

1. 在需要观察时启动本机 Navicat，先用测试数据库准备目标操作场景。
2. 运行下面的窗口枚举脚本，获取当前 PID 和窗口 ID。重新打开窗口后应重新枚举。
3. 将 REA 活动目标切到原始 Navicat 应用，再按当前工具 schema 调用 `observe_native_ui`。
4. 优先记录一个明确窗口的截图和控件树；需要点击、输入或滚动时，使用声明了步骤的 `capture_native_ui_scenario`。
5. 记录操作前状态、动作、操作后状态，以及测试库中的实际结果。

```sh
/usr/bin/swift /Users/richie/.local/share/rea/navicat-handoff/list-navicat-windows.swift

# 用上一条返回的实际数字替换 PID 和 WINDOW_ID。
# /Users/richie/.local/bin/sqlshop-rea observe-native-ui \
#   '/Applications/Navicat Premium Lite.app' --pid PID --window-id WINDOW_ID --json
```

截图和控件树可能需要 macOS“屏幕与系统音频录制”及“辅助功能”权限。出现实际权限拒绝时，再为启动工具的对应应用或进程授权并重试；重新安装 REA 不能解决系统权限问题。

当前 Navicat 签名包含 hardened runtime，权限中未发现 `com.apple.security.get-task-allow`。LLDB 动态函数追踪尚未验证，不能把它当作已就绪能力。先使用静态证据和正常界面操作，不修改 Navicat 签名或安装包。

## SQLShop 当前状态

工作区为 `/Volumes/0/WorkSpace/sqlshop`，基线 HEAD 是 `95b923a`（`Polish platform-specific window chrome`）。现有项目使用 Rust、egui/eframe，支持 MySQL、PostgreSQL、SQL Server。

已有能力包括连接与 SSH、多连接、SQL 标签、草稿和查询库、表分页筛选排序、暂存编辑与提交、CSV 导入导出、基础建表和结构修改。用户希望参考 Navicat 重建产品体验，并补齐可靠的日常数据库管理流程。

本轮已写入的六项修复及相关回归测试尚未提交：

| 修复 | 相关文件 |
| --- | --- |
| 非事务表写入拦截及删除入口状态 | `src/db/table_edit.rs`、`src/db/browser.rs`、`src/model.rs`、`src/ui.rs`、`src/ui/workspace_ui.rs` |
| PostgreSQL 删除列前检查约束依赖 | `src/db/structure_edit.rs` |
| CSV 单列 NULL 行保留、引号与记录边界统一解析 | `src/model.rs` |
| 主键和同一行其他字段合并为一次 UPDATE | `src/db/table_edit.rs` |
| 单元格改回原值时移除对应暂存项 | `src/ui.rs` |
| macOS 最小化图标使用 AppKit 自有像素存储 | `src/ui/window_chrome.rs` |

接手时先收尾并验证这些改动，尤其不要将它们当作已提交的独立版本。当前检查状态：

- `cargo test --locked`：通过，包括 CSV、编辑器撤销暂存及图标内存寿命回归用例。
- `python3 scripts/test-docker.py test-all`：三引擎九项联测通过，包括补充的组合主键批量修改、CHECK 依赖和 MyISAM 拦截场景。
- `git diff --check`：通过。
- `cargo clippy --locked --all-targets -- -D warnings`：尚未通过；`src/ui/window_chrome.rs:89` 的 `let Some(bitmap) ... else { return None; }` 触发 `clippy::question_mark`，需改为 `?` 后重跑。
- 原生 Dock 图标 GUI 测试尚未运行；默认测试验证了像素存储寿命。
- 查询取消、独立查询会话和显式事务控制尚未实现。

只使用专用测试库。`/Volumes/0/WorkSpace/sqlshop/.env.test` 已存在，口令留在本机；使用既有脚本读取，不把口令、连接配置或真实业务数据写进规格、截图说明或 Git。

## 重建顺序和交付物

### 第一阶段 交互规格

先覆盖以下流程，并把 Navicat 的实际行为与 SQLShop 的设计选择分开记录：

| 流程 | 需要回答的问题 |
| --- | --- |
| 主窗口与对象树 | 连接、数据库、schema、表如何展开和选中；单击/双击；搜索；右键菜单；空状态 |
| 查询工作区 | 标签与执行目标绑定；执行选区/当前语句/全文；结果和错误；快捷键；忙碌状态 |
| 表数据编辑 | NULL 与空串；新增；多格暂存；提交/撤销；主键修改；冲突；刷新；离开标签的处理 |
| 会话与事务 | 同连接不同数据库的隔离；自动提交；未结束事务；取消/断开后的反馈 |
| 表结构 | 字段、类型、默认值、约束、索引；修改预览；依赖拒绝；实际 DDL |
| 导入导出 | 列映射、类型转换、默认值、NULL、失败定位、进度与完整数据导出 |

产出建议：

- `docs/navicat-reference-spec.md`：截图/证据索引、界面状态和流程规则。
- `docs/sqlshop-rebuild-plan.md`：现状差距、实现顺序、涉及模块和每项验收方法。

每条关键规则记录来源：实际界面观察、数据库结果、静态分析，或 SQLShop 自行选择的设计。资源中出现一个菜单项不代表功能已验证。第一阶段完成后再开始大范围 UI 重构。

### 第二阶段 日常主力流程

保留已验证的数据库层，按以下顺序实现：

1. 收尾当前修复和质量检查，保证错误消息与实际写入结果一致。
2. 统一主窗口、对象树、标签和数据网格，补齐键盘操作和空/忙/错状态。
3. 调整会话模型，支持查询会话与表编辑/目录操作隔离；显示明确的事务状态。
4. 实现查询取消及结果反馈，区分取消、错误与提交结果未知，不自动重试写入。
5. 完善 SQL 编辑：当前语句执行、格式化、表名/schema/别名补全、错误位置。
6. 补齐表结构设计和导入导出的常用操作。

继续沿用 Rust 和现有三引擎支持。若 UI 实现方式需要重大调整，先在计划中说明可验证的问题、替代方案和迁移成本，不因 Navicat 使用另一套原生框架就直接替换整个技术栈。

### 第三阶段 管理能力

日常流程稳定后，再扩展执行计划、锁与会话诊断、视图/函数/存储过程/触发器管理、原生备份恢复，以及签名安装包。未在 Lite 版验证的高级功能要单独立项，不依据资源名称宣称已复现。

## 验收与工作区约束

核心验收流程是“连接 → 查询 → 编辑 → 提交/撤销 → 验证 → 导出 → 错误后恢复”。至少覆盖两种并发操作、切库、断网、主键冲突、NULL/空字符串、大表和非事务表。

每项实现都应保留数据库真实结果的验证，避免只做截图或只测生成的 SQL 字符串。根据改动运行必要检查：

```sh
cd /Volumes/0/WorkSpace/sqlshop
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/test-docker.py test-all
```

不要把以下文件提交到仓库：Navicat 安装包或 ARM64 副本、反编译全文、REA/Ghidra 运行目录、配置备份、口令、真实业务截图或数据。可提交自行编写的规格、实现、测试以及经检查的参考截图说明。二进制与大体积证据保留在上述本机目录，Git 中使用来源索引。

环境安装不授权接手智能体修改真实业务数据库、发布软件或改变 Navicat 本体。重建测试在专用测试环境进行；需要额外外部操作时沿用用户实际授权范围。

## 给接手 Codex 的任务提示

```text
请先阅读 /Volumes/0/WorkSpace/sqlshop/docs/navicat-rebuild-handoff.md。
REA 6.4.0、Ghidra 12.1.4 和 Codex MCP 已配置；重启后的工具目录为准。
分析目标是本机 Navicat Premium Lite 17.3.11，已有证据在文档指定目录。

先检查 SQLShop 的未提交改动并收尾已有质量问题。随后完成第一阶段：
用 REA 和专用测试库分析 Navicat 的主窗口、对象树、查询标签和表数据
编辑流程，产出可验证的交互规格与分阶段重建计划，再按计划实现。

保留 SQLShop 的 Rust 数据库、SSH、存储及三引擎测试基础。区分观察、
推断和自行设计；尊重证据覆盖范围，不把伪代码当作原始源码。
```
