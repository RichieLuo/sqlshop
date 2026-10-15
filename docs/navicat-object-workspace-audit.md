# 对象工作区：原始资源与目录内容审计

## 导航树交互补充（2026-10-15）

本次复用N7有界反编译并重读本机原包帮助，没有启动或登录Navicat。`Info.plist`仍为17.3.11；本次重新计算主二进制SHA-256，仍为 `47b428be0693e9cb353ea1ba1ef056d6075b1e2fcd944b60d632da63f342a1f6`，与N7归档目标相符。

| 已知行为 | 精确依据 | 对SQLShop的处理 |
| --- | --- | --- |
| 数据库/schema双击打开；表左箭头展开元素；具体元素双击进入设计器 | 原包 `Navicat Help/navigation_pane.html:20–23`，`preferences_gen.html:24–29` | 单击选择与明确激活分开，箭头一路展开即可浏览；分类容器双击只展开/折叠，具体叶子才打开设计 |
| 已展开节点双击走折叠；其余节点按owner override或节点类型打开 | N7 `output/NAVConnectionTreeViewController_ibConnectionTreeDoubleClickAction_.c:19–47` | 连接/库/schema容器双击切换展开，折叠不强制切对象页；表保留双击开数据，是稳定操作入口的项目选择 |
| 数据库有子项可直接展开，否则请求打开；普通对象走打开对象链，连接按状态展开或连接 | N7 `output/NAVConnectionTreeViewController_openConnectionTreeItem_.c:30–114` | 展开时按需连接/读取，未加载分支显示状态与明确重试入口，不要求先单击名称再展开 |
| 选择更新浏览、信息和状态；打开对象为独立链 | N7 `output/NAVConnectionTreeViewController_outlineViewSelectionDidChange_.c:16–43`及 `MainObjectViewController_connectionTree_selectionDidChange_.c:876–884` | 普通选择只更新浏览模型，不强制离开活动文档；右键、拖动释放、箭头不再走通用打开路径 |

以上N7文件相对 `/Users/richie/.local/share/rea/navicat-handoff/navicat-workflow-bounded-ghidra/`。owner override、所有节点数值映射与原应用实际可见标签切换仍未完全恢复。`navicat-objc-metadata.txt:1101626–1101628`存在树键盘handler（IMP `0x10125c55c`），本轮没有其函数体；SQLShop新增上下/左右/Home/End与Enter规则属于项目交互，不作为Navicat键盘复刻证据。在导航帮助、`preferences_*.html`及中文本地化的有界范围未找到单击展开偏好，不据此断言所有版本不存在该选项。

本轮通过技能要求的独立证据/当前交互审查定位了混合整行折叠、行尾特殊命中与鼠标释放强制跳页，采用统一组件修复。没有新增或运行测试、没有执行数据库操作；检查范围见主清单。

日期：2026-10-14。针对用户指出的“对象页内容太少”，沿用既有 N4 / N7 静态证据，补读原应用随附的中文帮助和本地化资源。没有启动 Navicat、登录、连接数据库或运行测试。

## 证据身份与边界

- 原应用：`/Applications/Navicat Premium Lite.app`，17.3.11；原始二进制 SHA-256：`47b428be0693e9cb353ea1ba1ef056d6075b1e2fcd944b60d632da63f342a1f6`。
- N4：REA `ev_4c9d78c1d78291fea95f47b08a4446ae812ff159942fa868dcf1b80e4c89c43d`；复用 `/Users/richie/.local/share/rea/navicat-handoff/navicat-focused-ui-summary-20261014.json`，原始资源匹配、所选子集未截断。归档控件及 action 连接不能证明运行时启用或像素位置。
- N7：复用 `/Users/richie/.local/share/rea/navicat-handoff/navicat-workflow-bounded-ghidra/` 精确函数范围、已有反编译及汇编。未重复全量反编译。
- H2：原应用 `Contents/Resources/zh-Hans.lproj/Navicat Help/` 产品文档。它覆盖多个版本和版本等级，不能据此断言 Lite 已启用所有功能。
- L2：原应用 `Contents/Resources/zh-Hans.lproj/*.strings` 的静态标签。属性标签存在不等于所有引擎的对象页都显示这些列。

## 对象页的实际组织

| 原始事实 | 来源 | 对复刻的直接要求 |
| --- | --- | --- |
| 对象页可以列出表、视图、查询等，主工具栏选择对象类别，对象工具栏执行该类别动作 | H2 `main_window.html:20–38` | 浏览范围包括实际对象类别和本地查询库；仅有表名列表内容不足 |
| 详细信息视图按列显示对象名称和属性，列可按对象类别选择；列表或详细信息可隐藏对象组 | H2 `object_pane.html:20–23` | 增加真实目录属性、列选择及分组 / 平铺；搜索覆盖名称和已加载属性 |
| 对象浏览宿主是 `NAVOutlineView`，有表头宿主；`Detail`、`Flatten Object List`、`Choose Columns…`、`Show Hidden Items` 均连接 `ibViewMenu:` | N4 `MainObjectViewController.nib`：对象 13、17；菜单 141、150、154、159 | 详细列表、分组与列显隐有静态原始依据；归档中空表头不能推断具体默认列 |
| 对象选择会装入对象类型、holder 和行数组，并更新信息视图及状态；对象类别装载依赖连接类型和服务器版本 | N7 `loadMainObjectViewForNode_`、`displayTreeNodesForSchema_type_options_` | 类别和属性应由真实服务器能力决定；选择信息与打开文档分别处理 |
| 列表或详细信息双击表打开数据查看器，ER 视图双击打开设计器 | H2 `object_pane.html:39` | 保持单击浏览、双击打开数据；设计动作另有入口 |
| 表下可展开字段、索引、外键，双击这些元素打开设计器；设置可隐藏表下对象 | H2 `navigation_pane.html:20–23` | 树的子对象信息可后续按需加载，避免刷新时逐表全量查询 |

ER 图表在 H2 `object_pane.html:24` 明确限定 Premium、企业版和标准版。不能为填满页面而添加没有功能的 ER 按钮，也不能将其算作已复刻 Lite 的证据。

## 引擎对象类别

下面是打包帮助直接声明的产品范围。它列出参考对象，不表示本项目已经实现其设计器或管理操作。

| 引擎 | 类别与位置 | 原始文档 |
| --- | --- | --- |
| MySQL | 表、视图；“函数”列表同时收录过程与函数；“其他 → 事件”列出计划事件 | `mysql_table.html:20`、`mysql_view.html:20`、`mysql_function.html:20`、`mysql_event.html:20` |
| MariaDB | 继承 MySQL 对象；“其他”内的序列仅 MariaDB 10.3 或更高 | `mysql_others.html:20–22` |
| PostgreSQL | 表、视图、实体化视图；“函数”列表同时收录过程与函数，文档注明过程需 11 或更高 | `postgresql_table.html:20`、`postgresql_view.html:20`、`postgresql_mview.html:20`、`postgresql_function.html:20` |
| PostgreSQL | “其他”包含聚合、转换、域、索引、运算符、运算符类别、序列、触发器、表空间、编制及语言 | `postgresql_others.html:20` |
| SQL Server | 表、视图；“函数”列表同时收录过程与函数 | `sqlserver_table.html:20`、`sqlserver_view.html:20`、`sqlserver_function.html:20` |
| SQL Server | “其他”包含索引、同义词、触发器、备份设备、链接服务器、服务器触发器、程序集、数据库触发器、分区函数及分区方案 | `sqlserver_others.html:20` |

函数分类可以拆分为更直接的“函数 / 过程”入口，但这是项目的呈现决定；原帮助的顶层“函数”包含两者。PostgreSQL 重载函数必须保留参数签名或服务器 OID 身份，不能按 schema + 同名合并。触发器和索引必须保留所属表，不能按同名对象合并。

## 信息内容与属性

H2 `information_pane.html:20–57` 明确：信息页随选中对象变化，提供常规、DDL、查询预览、依赖的对象、使用该对象的对象、标识符及字段等不同内容。其中“标识符”用于数据库 / schema 下的表、视图和字段，“字段”用于表查看器选中字段。权限、角色成员、项目活动是特定上下文，不应固定放在每个表旁边作为空栏目。

L2 `Localizable.strings` 真实存在的属性键包括：`Estimated Rows`、`Data Length`、`Index Length`、`Average Row Length`、`Storage Engine`、`Collation`、`Comment`、`Owner`、`Created Date`、`Modified Date`、`Last Analyzed`、`Row Format`、`Tablespace`、`Storage Size`。当前静态范围没有逐引擎对象列组装函数，因此只能证明产品有这些标签，不能证明对象列表默认排列或每个对象都适用。

复刻实现应采用服务器能提供的信息：

- 表：schema、类型、注释、估算记录数、数据 / 索引占用；引擎支持时展示存储引擎、排序规则、创建 / 修改时间、所有者。
- 视图、函数、过程、触发器：schema、类型、定义语言或返回类型、签名、所属对象、注释；定义查询结果可用于 DDL / SQL 文档。
- 索引、序列：所属 schema / 表、索引方法、唯一性或序列数据类型、目录可用属性。
- 本地查询：命名、所属连接 / 数据库、SQL 预览和已有文档打开链，不伪装成服务器对象。

这些是本次实现建议，不是已经观察到的默认列。估算记录数必须标明“估算”；读取不到的值保持未知，不能填 `0`。不能为显示列表而对每个表执行 `COUNT(*)`。目录权限不足或某类查询失败必须单独提示，不能将失败伪装为零对象。

## N7 精确引用

| 函数 | 原始范围 | 已保存反编译的关键位置 |
| --- | --- | --- |
| `MainObjectViewController_loadMainObjectViewForNode_` | `[0x1013ae698, 0x1013aebc4)` | `output/MainObjectViewController_loadMainObjectViewForNode_.c:46–49` 装入行数组；`92–98` 对象类型和父 holder；`152–168` 虚拟组以及信息 / 状态更新 |
| `MainObjectViewController_displayTreeNodesForSchema_type_options_` | `[0x1013477b0, 0x101347c08)` | `output/MainObjectViewController_displayTreeNodesForSchema_type_options_.c:61–69` 连接与版本决定类型；`94–125` 当前数据装入及后台目录任务；`130–135` 工具栏与信息更新 |

范围及 selector 为已有静态解码事实；未解码的数值 objectType / nodeType 不能凭名称猜成具体类别。

## 本次引用文件摘要

下面均相对原应用 `Contents/Resources/zh-Hans.lproj/`。原始文件未修改。

| 文件 | SHA-256 |
| --- | --- |
| `Navicat Help/object_pane.html` | `94fbf9a2cced4a30a791f4db5d4df61b1c3e5c3de5bf457e2f52ae26a802cccb` |
| `Navicat Help/information_pane.html` | `78d84b3c3cc160c8b144543c180fe1784a86537ea0e425b6e42fc7828566e7f0` |
| `Navicat Help/mysql_function.html` | `9ae8a02b875df98dfb224432a58fe1b8159aec5b6a6067dabb794aff512f1b57` |
| `Navicat Help/postgresql_function.html` | `c8ccfeadbff4fdcf6fa4413c75e94580612f2b2a38c7936845af84f7a4415ed7` |
| `Navicat Help/sqlserver_function.html` | `31b46db1ec4f5f24ac5930cc556a326b1d77dc8c9af4b3abc78eb5ca5941a65f` |
| `Navicat Help/mysql_event.html` | `f5b71252b88e136b0239388d01ddf3651f97ae357e4cc36a72e91a18b48e62c4` |
| `Navicat Help/postgresql_mview.html` | `57d54c803f6ff9876757caa02189ba6ba3c0e38f4d3f51017716adf774613dc7` |
| `Navicat Help/mysql_others.html` | `e378c89dc2ee35d1f436a9cfeb4890560f261afa3e41e316e54373795d3b4b01` |
| `Navicat Help/postgresql_others.html` | `f7545a06905d26389bf03f7c976cdf3e69829288af67b16d1cf86ec34a0df3f1` |
| `Navicat Help/sqlserver_others.html` | `c3a258297e410784e2318fdb67187118f4aacf9ecdddc34eb061ecece6d9c5e2` |
| `Localizable.strings` | `93463f0c63e02a3efc5286d7e271f457d4958fe348583b6e948147c316a66ec8` |
| `MainObjectViewController.strings` | `769a8b6a90500e153dab18b21b28ce73e2b988b6c37409ae3b2e4fbb8d348c32` |

`.strings` 是带 UTF-16 BOM 的 XML，其 XML header 声明 UTF-8；本次先按 BOM 解码再交给 plist 解析器，未执行任何 HTML 脚本。
