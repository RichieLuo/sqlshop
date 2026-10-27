# 已保存查询的归属与入口

日期：2026-10-26，Asia/Shanghai。

本次修正连接树顶部的全局“已保存查询”列表和独立查询库窗口，将查询放进对象浏览工作区。

## 参考证据

- 本机 Windows Navicat Premium 17.2.7.0：`C:\Program Files\PremiumSoft\Navicat Premium 17\navicat.exe`，SHA-256 `162c89e00f8ee99b668063bbfa83df2fc41ca786b1f709ff22fb53955f24d34d`。
- REA `inspect_pe_resources`：`ev_0248440371bc66be08b6cf4962736ee6f8516ce003f9cd87bf42d3fa57b9907b`，完整枚举 2,530 项资源。`TNAVICATMAINFORM` 位于文件偏移 72,704,688，长 169,798 字节，SHA-256 `b9c64631694cca673996703d1eab67676f1353c7c4577f188a32ac8e197208ee`。其中可读动作包括 `acGroupQuery`、`acGroupQueryExecute`、`acDesignQuery`、`acNewQuery`、`acDeleteQuery`。
- `TQUERYSAVEDIALOG` 位于文件偏移 73,183,404，长 1,621 字节，SHA-256 `ff7d20913667ec8e3d88fdb3f29831fc9e79b6eb08d7b8c6322ee930aa7cb0bb`。资源含名称输入 `edUserInput`、保存位置 `laPath` 和 `cbConn`、`cbCatalog`、`cbSchema` 控件。只据此确认静态控件存在，不推断默认路径或执行逻辑。
- 同一安装目录的 `navicat.pdf`：第 27–28 页说明主窗口对象窗格显示表、视图、查询列表；第 117 页明确点击主窗口“查询”打开查询的对象列表；第 118 页区分外部 SQL 文件与 Navicat 查询的保存。此为随包手册的行为描述，不是原程序运行观察。
- 完整 REA Evidence：`C:\Users\adm\.codex\visualizations\2026\10\26\01a176c4-9c6d-7d12-9928-7551a565ecfa\navicat-query-placement-rea.json`。

本轮没有启动 Navicat，没有执行全程序反编译；不声称恢复了原应用的精确树节点、像素位置、默认保存目录或云端机制。

## SQLShop 的实现

- 连接树顶部恢复为搜索和连接导航。连接、数据库及 Schema 下的“查询”节点打开中央对象区；中央对象分类中，“查询”与表、视图等并列。主工具栏“查询”和查询菜单“查询列表”也进入同一工作区。
- 连接级显示该连接的查询，数据库级按数据库过滤，Schema 级再按 Schema 过滤。范围使用保存的目标，不读取活动 SQL 标签的运行状态来猜测归属。
- 单击选中并显示 SQL 预览，双击或“打开查询”进入编辑标签。搜索覆盖名称和 SQL；列表和右键菜单提供重命名与删除，沿用原有确认及过期写入保护。
- 本地查询列表可离线浏览。“全部连接”包含未绑定查询和已删除连接的旧查询；从未绑定文档进入查询列表时自动显示全部。新建查询继承当前浏览范围，从全部连接新建则保持未绑定。
- 查询仍存于原来的 `catalog.json`，ID、SQL、执行目标和工作区恢复格式不变。连接分组管理保留独立窗口；查询编辑器只保留保存相关入口。

这些是依据参考流程作出的 SQLShop 设计选择。没有增加 Navicat 的文件目录格式、云同步或对象组机制。

## 验证

已通过 24 项相关测试：查询库 11 项、对象浏览与查询列表 7 项、树导航 6 项。覆盖目标范围隔离、未绑定查询、保存与重命名后的文档身份、草稿保护、离线单击选择及双击打开。交互检查修正了名称标签的文本选择对整行双击的拦截。

全目标编译检查、全目标 Clippy（`-D warnings`）、构建、格式检查、差异格式检查和源码体积检查通过。上游 PostgreSQL 驱动原有四条警告仍保留。

本轮已成功构建 `target/debug/sqlshop.exe` 并完成原生截图检查。之后重复构建期间，本机 Visual Studio 安装器日志 `dd_setup_20261026140538.log` 记录在 14:09:55 完成卸载，原有 MSVC / Windows SDK 库随之缺失；当时重复构建因 `opengl32.lib`、`ucrt.lib` 不可用而失败。随后已按下方正式包记录，通过独立构建依赖恢复正式构建。本轮没有安装、卸载或修改系统工具链。

交付程序 SHA-256：`52559b37104a91ae6da348254a885a86811f2e01676a5d3dc4df61fa37e9b812`；工具链变化后再次运行该程序，两种离线截图均成功生成。

原生离线布局已检查 1280×820 浅色、900×600 深色；截图位于上述 Evidence 同目录的 `saved-queries-light.png` 与 `saved-queries-narrow-dark.png`。截图仅用隔离示例数据，不读取用户配置、不建立数据库连接。没有做真实数据库验收，也没有替换或关闭用户正在运行的安装版窗口。

## 正式包与开始屏幕入口

用户后续明确要求以原来的开始屏幕入口为准。本轮已构建并安装 **SQLShop 1.0.0，Windows x64 Release**，包括上述查询位置调整以及工作区当前改动。

- 正式包：`dist/saved-query-placement-20261026/SQLShop-1.0.0-windows-x64.zip`；SHA-256 `1fe8a71236a6601113468e07742167f30693639455b4e755f9e5c06e23ffd008`，旁边附 `.sha256` 校验文件。
- 程序 SHA-256：`616ba5eb9479fffcb77027781ba92fe061c8f7145bc8548cb0397170da74ffd8`。程序资源版本为 1.0.0，VC 运行库静态链接；导入表检查未发现 VC 运行库 DLL 依赖。
- 固定入口继续使用 `C:\Users\adm\AppData\Local\Programs\Shop\SQLShop\SQLShop.exe`。原快捷方式仍为 `C:\Users\adm\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Shop\SQLShop.lnk`，没有改成工作区调试程序。
- `current.path` 已指向 `versions/1.0.0-616ba5eb9479-425b9fc268e0/sqlshop.exe`。已校验 ZIP、包元数据、安装副本完整指纹及启动器哈希一致，并从原快捷方式启动，确认进程路径属于该新版本目录。旧版本目录保留。
- 打包与安装脚本测试 22 项通过、2 项按现有条件跳过；安装后的正式程序成功生成 `release-saved-queries.png`，随后通过原快捷方式正常启动。

可复用的构建环境脚本为 `C:\Users\adm\.local\share\sqlshop-build-tools\20261026\Use-SQLShopToolchain.ps1`。它在当前构建进程中组合本机 MSVC 编译器、保留的 SDK 头文件及独立解压的构建库，不修改全局环境或 Visual Studio 安装。CRT 来源与 SHA-256 对照本机 Visual Studio 官方安装目录的 catalog；Windows SDK 库来自 [Microsoft.Windows.SDK.CPP.x64 10.0.26100.7705](https://www.nuget.org/packages/Microsoft.Windows.SDK.CPP.x64/10.0.26100.7705)。下载来源和文件哈希记录于同目录 `sources.json`。

复用本机构建环境时，先在 PowerShell 执行 `. 'C:\Users\adm\.local\share\sqlshop-build-tools\20261026\Use-SQLShopToolchain.ps1'` 加载环境，再运行 `scripts/package.ps1 -Target x86_64-pc-windows-msvc -OutputDirectory <新的输出目录> -Install`。现有版本化安装流程会同步更新同一个开始屏幕入口。
