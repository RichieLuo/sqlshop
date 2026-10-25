# 本轮结构调整

本轮保留原业务公共接口、数据格式和配置目录，拆开混合职责与测试。统计为物理行数，包含注释和空行；以下是原大文件与现在入口的对照，不代表删除了这些功能。

| 原文件 | 调整前 | 现在入口 | 调整后 |
| --- | ---: | --- | ---: |
| `src/ui.rs` | 11060 | `src/ui/mod.rs` | 1006 |
| `src/ui/workspace_ui.rs` | 2453 | `src/ui/workspace_ui.rs` | 677 |
| `src/ui/redis_ui.rs` | 2244 | `src/ui/redis_ui.rs` | 840 |
| `src/ui/object_browser_ui.rs` | 2099 | `src/ui/object_browser_ui.rs` | 828 |
| `src/sql_editor.rs` | 1750 | `src/sql_editor.rs` | 298 |
| `src/worker.rs` | 1381 | `src/worker.rs` | 830 |
| `src/db/table_edit.rs` | 1428 | `src/db/table_edit.rs` | 732 |
| `tests/redis_database.rs` | 1863 | `tests/redis_database.rs` | 217 |

- `src/ui/mod.rs`：入口保留 eframe 生命周期；startup、commands、events、shell、workspace、dialogs、grid 等子模块各自承担单一分区；UI 测试按编辑、网格、工作区、会话、导航等行为分组。

- `src/ui/workspace_ui.rs`：拆出文稿、工作区持久化、查询工具条、表操作与确认窗口；不再将文件保存和表格操作堆在同一实现中。

- `src/ui/redis_ui.rs`：状态和请求留在入口，键浏览、值编辑及确认窗口分别独立；测试沿用同一 RESP mock 行为。

- `src/ui/object_browser_ui.rs`：浏览数据、列表/图标视图与属性面板分离。对象动作由 host 负责，定义文档和动作分发也分别拆出。

- `src/sql_editor.rs`：语句边界、语法高亮、补全和格式化拆为四个模块，保留原公共函数路径。

- `src/worker.rs`：目录/表工作线程与每个查询标签的独立工作线程分离，共用取消错误类型仍放在共同入口。

- `src/db/table_edit.rs`：表读取、字段输入检查与事务写入分离；错误类型与事务失败语义保持不变。

- `tests/redis_database.rs`：将协议、认证、数据库元数据、命名空间、变更和边界测试分组；mock 服务及 SSH 支持保留在集成测试入口。

所有新增子模块通过正常的 Rust 模块声明连接，不使用 include! 拼接源码。测试仍由 cargo test 自动发现；忽略的实库/显式样本测试保留原忽略条件。

运行 `python scripts/check-structure.py` 查看当前最大文件与 CI 检查线。vendor 驱动补丁不做纯风格重排，以保留上游差异可审阅性。

## 验证记录（2026-10-25，Windows x64）

- `cargo fmt --all --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked` 通过；原有显式忽略的实库/调用方样本测试仍按原条件跳过。
- `python -m unittest discover -s scripts/tests -v` 的 7 个打包契约测试通过，覆盖失败构建、目标目录、归档权限、静态 CRT 参数、旧包保护和版本 tag 匹配。
- `python scripts/check-structure.py` 与 `git diff --check` 通过。
- 对照 Git 原版本核对函数声明，未发现原有函数遗漏；294 个 Rust 测试声明保持一致。SQLShop 的图标像素函数统一命名为 `icon_image`。
- Windows 发布包实际构建完成，ZIP 完整性、包内程序 SHA-256、使用说明和第三方许可清单核对通过；PE 版本资源与项目名称/版本一致，依赖列表不包含动态 MSVC CRT。
- 三仓库的通用打包脚本、资源环境脚本、文件规模检查、发布工作流、架构文档及格式配置保持一致。

本次生成的候选包为 `dist/architecture-unification/SQLShop-0.1.0-windows-x64.zip`，SHA-256 为 `6c6ff2a672b27c2d2c577db6577c535ebb25b8930d99b0f8cc2687029d80681f`。此目录是验证输出；常规打包仍默认使用 `dist/`，已有同名产物不会被覆盖。

本轮未执行 GitHub Actions、macOS/Linux 构建或目标系统启动验证。跨平台 CI 和发布配置已统一，不能将本机 Windows 验证视为三系统实测。
