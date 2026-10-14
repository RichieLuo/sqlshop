# Navicat ARM64 二进制分析记录

日期：2026-10-14，Asia/Shanghai。按用户指定的直接反编译路线推进，无需登录或执行原应用。现有 SQLShop 功能可以按参考实现效果重写和替换。

## 目标与证据

分析目标：`/Users/richie/.local/share/rea/navicat-handoff/navicat-arm64`，SHA-256 `f200508a133605213e0b7bc0bec9dd670f48d495f79c5994a1da9ac2ef5e359d`。来源为本机 Navicat Premium Lite 17.3.11 的 ARM64 切片；原应用未修改。

以下地址是该切片的未滑动虚拟地址，不能作为 x86_64 地址、原始文件偏移或运行中 ASLR 地址使用。

本机 `navicat-table-actions-symbols.json` 保存 `otool -ov` 和 `nm -n` 相互印证的 Objective-C 方法记录。它们是普通工具的静态观察，不冒充 REA 反编译证据。

| 真实方法 | ARM64 IMP |
| --- | --- |
| `-[TableViewViewController ibRefresh:]` | `0x101229b48` |
| `-[TableViewViewController ibStop:]` | `0x101229e08` |
| `-[TableViewViewController ibPostAll:]` | `0x101229fec` |
| `-[TableViewViewController ibCancelAll:]` | `0x10122a0b0` |
| `-[TableViewViewController ibSetNullValue:]` | `0x10122cce4` |
| `-[TableViewViewController ibSetEmptyStringValue:]` | `0x10122cd60` |

该类父类为 `NAVTabItemViewController`，其父类为导入的 `NSViewController`；该类方法清单含 490 项。方法存在不单独证明 Lite 的菜单启用、运行执行或事务边界。

## 静态资源与源码对应

REA 定向资源证据：`ev_4c9d78c1d78291fea95f47b08a4446ae812ff159942fa868dcf1b80e4c89c43d`。9 份原始 NIB 的 SHA-256 已与定向派生资源逐一匹配。具体来源与范围见 `navicat-reference-spec.md`。

`TableViewViewController.nib` 的单格/多格菜单分别连接 Set to NULL → `ibSetNullValue:`、Set to Empty String → `ibSetEmptyStringValue:`。底部操作条包含新增、删除、提交全部、撤销全部、刷新、停止，右侧有记录和分页、Grid/Form 与设置。这些是归档中的结构和连接，不代表动态布局或功能可用状态。

SQLShop 下一步据此增加直接暂存 NULL/空字符串、撤销当前格和键盘进入编辑，保留原始主键与原值、事务提交和只读保护。入口与参考静态资源对应，暂存和错误恢复仍按 SQLShop 已验证的数据库流程实现。

## 反编译调查清单

主程序全量自动分析曾长时间停留于全局 Switch 识别，已主动结束并关闭 REA 会话（记录 `navicat-full-analysis-stop.json`），没有将未完成结果当作反编译证据。改为依据已核对函数边界的独立有界 Ghidra 分析。后续按真实地址调查：

1. 提交/撤销是否先结束当前编辑；如何处理多行修改和失败。
2. 刷新是否检查修改状态；取消与刷新如何影响当前数据模型。
3. NULL/空字符串是否针对选区批量调用编辑模型；条件与只读检查位于何层。
4. 下游保存对象、数据库执行、错误传递与状态更新的实际调用关系。

结论须附函数地址及 REA Evidence ID；伪代码和 Objective-C 间接调用无法确定的部分标为推断或未知，不声称还原原始源码。

## 机器码与下游模型的已确认关系

下面结论来自有界 ARM64 反汇编、selector stub 的 `adrp/ldr` 引用、Objective-C ivar 和方法元数据。完整片段与调用索引保存于本机 `navicat-table-action-downstream-index.json`、各 `*-assembly.txt` / `*-static-calls.json`。Ghidra 伪代码后续用于进一步核对；这些普通工具结果不冒充 REA Evidence。

- `ibPostAll:` 先同步编辑器，依据 Grid/Form 视图分派到对应控制器的 `postAll`；`ibCancelAll:` 分派撤销后再同步编辑器。
- Grid 的后台保存 block `0x1005e9460` 通过 `dataFormDataForGridVC:` 取得数据模型，调用 `postAll:`，将返回 byte 捕获给主队列 block `0x1005e959c`。
- 主队列 block 先检查保存返回值是否为 1。只有成功分支对 `_arrayOfPastedRow`（`NSMutableIndexSet`，实例偏移 `0x100`）调用 `removeAllIndexes`，并设置 `setMultiRowChanging(false)`。这是粘贴行索引清理，不能等同于全部修改草稿清理。`didPostDataOnGridView:` 只受 `respondsToSelector` 检查保护；重绘、`_skipPosting`（偏移 `0x1a0`）清零及 `enableUserInteraction(true)` 位于正常返回路径，不依赖保存成功。因此回调本身不能当作保存成功证据，字段清零也不能未经核对称为清忙碌状态。
- `dataFormDataForGridVC:`（`0x101238290`）与 `dataFormDataForFormVC:`（`0x101237b14`）都返回控制器 `_formData` ivar。该 ivar 元数据声明类型为 `DataFormBaseData`，实例偏移 1048。
- `DataFormBaseData postAll:`（`0x1009f7c58`）转发至 `postAll:forMain:`（`0x1009f7c60`）。后者检查 dataset 是否 editable，再分派到导入的 `postAllAfterExecute(ICSForm*,CMObject*)` 或 `postAllTableViewForm(ICSForm*)`；`nm -m` 将这些导入绑定至 `libcc`。Grid 控制器传入标志位来自 `_hasMultiDataset`（偏移 `0xdf`），不是根据变量位置猜测“自动提交”。底层事务原子性尚不能由此确认，需要继续检查导入库实现。
- 刷新先结束 Grid 当前编辑并保留选中行索引；刷新 block `0x101229c84` 调用导入的 `CCNavicat::refreshTableViewForm(ICSForm*)`，再安排主队列更新。原应用刷新后的精确选区行为仍未运行核对。
- NULL/空字符串操作根据当前 Grid/Form 视图分派，分别调用 Grid 的 `setNull`/`setEmptyString` 或 Form 的选中列操作。当前入口本身不证明底层约束与只读检查的完整覆盖。

实现对应：SQLShop 已按成功响应清理草稿、错误保留草稿，并保护提交期间的编辑/撤销。新增直接 NULL/空串和单格撤销沿用同一草稿模型。后台保存与 UI 状态分离，不以一个通用“操作完成”回调清理草稿；底层事务继续由已有三引擎真实联测保证。

## libcc 的实际保存实现入口

原始依赖库为 `/Applications/Navicat Premium Lite.app/Contents/Frameworks/libcc.dylib`，SHA-256 `32d72fee0a0004a30c3aee1d245f94d7ad6876e4a581b0ef72ac32f3dd30f0a4`。本机派生 ARM64 工件 `libcc-arm64` 的 SHA-256 为 `32b04a9453cca8d8532a86797826f82a8340478001a02aa596a1d4421d25f01a`。下面地址均属于该库切片，不能与主程序地址混用；证据为本机 `libcc-post-rtti-dispatch.json` 与有界反汇编。

| 导出入口 | 库内 VM 地址 | 已核对的实际实现链 |
| --- | --- | --- |
| `postAllTableViewForm` | `0x21052b8` | dynamic_cast → ICSTableViewForm 的 vslot `+0x1b0` → thunk `0x24fe334` → `CSTableViewForm` 实现 `0x24fdfb0` |
| `postAllAfterExecute` | `0x1bdf12c` | dynamic_cast → ICSQueryDesignForm/CMDataset 的 vslot `+0x78` → thunk `0x19a95ec` → `CSQueryDesignForm` 实现 `0x19a9474` |
| `beginTransactionTableViewForm` | `0x2104ec4` | 接口 vslot `+0x1b8` → thunk `0x24fe4b8` → `0x24fe348` |
| `commitTableViewForm` | `0x2104f90` | 接口 vslot `+0x1c0` → thunk `0x24fe664` → `0x24fe4cc` |
| `rollbackTableViewForm` | `0x210505c` | 接口 vslot `+0x1c8` → thunk `0x24fe75c` → `0x24fe678` |

RTTI/vtable 对应关系已核对：CSTableViewForm 的相关接口子对象偏移 192、vtable address point `0x47492b0`；CSQueryDesignForm 为偏移 88、address point `0x4655500`。表格保存 body 在 `0x24fe040` 检查下游返回 bit 0，为真时于 `0x24fe050` 调自身 vslot `+0x1d8`，该槽指向上述开始事务实际实现 `0x24fe348`。这是条件性进入事务开始实现的静态证据，尚不能证明所有批次原子提交或错误时全部回滚；其余执行器虚调用需进一步解析。

## 已完成的有界 Ghidra 反编译

`libcc-bounded-ghidra/profile.json` 保存目标 SHA-256、完整命令、脚本校验值、六个函数的 LC_FUNCTION_STARTS 区间、执行状态和输出校验值。6/6 函数成功生成伪代码，输出及局限为同目录 `output/`、`findings.json`。这是 Ghidra 12.1.4 的独立工具输出，不是 REA Evidence；源库与主程序均未执行或修改。

采用 `-noanalysis`，仅对已核对地址区间反汇编/定义函数，并用 `DecompInterface` 逐函数反编译。由此不依赖全程序自动分析；代价是部分类型、调用签名、间接调用及异常边未知。区间本身与原始 Mach-O 的真实函数边界一致，不能把未解析调用重命名为猜测的业务函数。

伪代码与前述汇编相互核对：表保存中央动作是执行器 vslot `+0x60`（调用点 `0x24fe0c8`），当前表包装函数没有逐行保存循环；实际循环在尚未解析的执行器中。开始事务通过执行器 `+0x68`、提交通过 `+0x70`；表保存包装函数没有直接调用已识别的提交实现。两导出包装将状态码 0 转为成功，正值进入错误处理。以上仍不足以证明原应用多行编辑的事务原子性。

主程序有界分析也已完成 12/12：六个 TableView 操作入口、DataFormBaseData 保存分派、Grid 保存方法、保存后台/主队列 block、取消 completion 与刷新后台 block。与核心库合计 18 个函数。来源为本机 `navicat-main-bounded-ghidra/profile.json`、`findings.json`；原始输出保存在 `raw-output/`，可读输出在 `output/`。

为正确表达 Objective-C stubs 与导入的返回值，额外依据已经核对的 selector refs、方法编码及 ARM64 调用方读取 `w0` 的实际指令做有限 ABI 注解，清单为 `annotation-manifest.json`。32 位标志返回是反编译展示注解，不能称作原始 C/Objective-C 类型；自动生成的 `raw_argN` 也不是原源码参数名。原始与注解输出均保留，函数范围和源码工件不变。loader 已解码的起始指令产生的 disassemble 提示与反编译失败分开记录。

主程序伪代码与汇编再次确认：Grid 主队列 callback 只在保存返回字节等于 1 时清理粘贴行索引；delegate 回调在该判断之外。取消 completion 没有先检查 `cancelAll:` 返回值再复位界面。SQLShop 因此继续明确区分数据库成功、失败和本地撤销，避免以界面复位推定数据库写入结果。

## 对象浏览、文档打开与查询目标的核心链（N7）

本轮复用独立 `NavicatMainBounded` 工程，以 `-process navicat-arm64 -noanalysis` 对 20 个核心入口和 8 个必要支撑函数逐段反编译，28/28 成功。目标仍为上述 SHA-256 `f200508a…5e359d` 的 ARM64 主程序。范围按原文件 `LC_FUNCTION_STARTS` 确定，并由 `nm`、Objective-C 方法/类元数据或父函数的 block invoke 指针印证；没有全程序自动分析，也没有执行原应用或要求登录。

本机证据为 `navicat-workflow-bounded-ghidra/profile.json`、`adaptive/profile.json`、`findings.json`，各目录 `output/` 与有界 `*-assembly.txt` 保存伪代码和机器码。profile 含完整命令、工件 SHA、脚本摘要、逐函数区间、状态和输出摘要。这是独立 Ghidra 输出，非 REA Evidence。selector 名来自文件内的 ObjC stub 引用；有限 ABI 注解只帮助表达调用方寄存器，`raw_argN`、未命名字段和间接调用不能当作原源码签名。已将已知抛异常/展开栈入口标为不返回，避免伪代码继续落入相邻函数。

| 阶段 | 真实入口或已核对 block | 已确定的静态关系 |
| --- | --- | --- |
| 树选择 | `NAVConnectionTreeViewController outlineViewSelectionDidChange:` `0x10125ca58` → `MainObjectViewController connectionTree:selectionDidChange:` `0x10134f3a0` | 取得选中节点并调用 owner 的选择回调，更新主对象浏览状态、工具条与信息面板 |
| 树双击 | `ibConnectionTreeDoubleClickAction:` `0x10125e500` → `openConnectionTreeItem:` `0x10125ee8c` | 已展开节点走折叠；未展开节点走 owner 的双击 override 或按节点协议/status 分派展开、打开连接、数据库、对象 |
| 对象列表 | `loadMainObjectViewForNode:` `0x1013ae698`、`displayTreeNodesForSchema:type:options:` `0x1013477b0` | 按 holder/对象类别生成 row data，交给 `loadDataWithCurrentObjectType:holder:rowDataArray:`；部分类别/状态走空列表、分组或专用视图 |
| 打开对象 | `connectionTree:openObject:` `0x10134e270` → `openMainOutlineViewItem:` `0x10146bf94` | 树打开与对象列表打开汇合到 row data / objectRef 的对象类别路由，不由一般选择回调直接创建查询文档 |
| 文档协调 | `CCCoreUIManager openObjectWithParam:isReadyToFetch:` `0x1009f4940`、`newObjectWithParam:isReadyToFetch:` `0x1009f41d0` | 按 objectType 找 behaviour，检查动作支持，查已有 form 或建立新文档，并选择目标标签宿主 |
| 表文档构造 | `+[CCUIFormBehaviourTools openDataFormWithParam:inTabWindowController:readyToFetch:]` `0x100d1411c` | 常规分支构造 `TableViewParam` → `TableViewViewController` → `NAVTabItem` → `addTab:`；该类方法由 Meta Class 元数据确认，`nm` 中该函数名已被删减 |
| 新查询上下文 | `MainWindowController findActionTargetHolder` `0x100b2a178` → `openQuickQuery` `0x10129c5c4` | 根据主浏览标签/树焦点或当前文档，明确选择 parent holder，再交给 manager 新建查询 |
| 查询文档构造 | `CCUIQueryFormBehaviour newObjectWithParam:inTabWindowController:readyToFetch:` `0x100161548` | 将 parentRef、vGroup 和 extraParam 写入 design form 参数，构造 controller 和 `NAVTabItem`，再添加标签 |
| 查询目标变化 | `QueryDesignViewController didSelectServer:catalog:schema:` `0x1006fcb48`，后台 block `0x1006fce90`、主队列 block `0x1006fd058` | 保存文档目标，禁用依赖控件，后台使用连接；主队列按 server/catalog 更新列表、默认项和 editor holder |
| 执行目标 | `runSQL:explain:` `0x1006f53c0` → 后台 block `0x1006f5a4c` | 从本查询文档控件选取目标，将目标与执行参数捕获到 operation；更新该文档 form 后调用导入的 `runQuery` 或 `explainQuery` |

主对象浏览模型明确持有 `_currentHolder`、`_objectListHolder`、`_listingObjectType`、`_listingNodeType`、最近选中的 server/catalog/schema 等状态。选择回调依据节点类别更新这些状态，并调用 `updateInformationView`。`loadMainObjectViewForNode:` 遇到 parent virtual group 时递归显示该组；catalog/schema 节点使用父 server 与当前对象类别建立列表。浏览 holder 与查询文档的目标分别保存，提供独立的导航和执行上下文。是否每次单击都强制把可见标签切回主浏览页，及 owner/通知的完整影响，尚未建立；不把模型更新当成已运行观察的前台切换。

对象打开具有独立身份规则。`CCCoreUIManager` 读取 `Tabs.AllowMultipleInstances`；有 unique instance key 且该选项关闭时，按 key/formRole 查已有 form。无 key 或允许多实例时建立新文档，特殊 profile/quick-open/no-lock 参数另有分支。查询 behaviour 的 identity 可来自 objectRef 或 `CCCoreUIManager::filePath`；没有这两者的新查询返回空 identity。因此不能把所有查询仅按连接或库合并成同一标签。该选项的默认值和已有 form 的完整前台激活过程仍未知。`NAVBasicTabControlWindowController addTab:`（`0x100252808`）则明确执行添加、窗口置前与选中新标签。

新查询的 parent holder 按操作上下文选择。当前标签是主对象浏览控制器，或 firstResponder 属于连接树时，`findActionTargetHolder` 从树选中节点找最近 schema，其次 catalog，再其次 server，并取其 ref。其他情况下，若当前文档实现 `CCCoreUIFormInfo` 协议，则取文档 object 的 backend holder。`openQuickQuery` 使用该 holder 作为 parentRef。这里的树焦点判断与文档 holder 分支有真实机器码支持，不能简化为“总是使用全局最近点击的连接”。

查询文档具有 `_serverPopUpButton`、`_catalogPopUpButton`、`_schemaPopUpButton`，以及各自 `_serverInUse`、`_holderRef`。选中目标后，依赖列表先禁用/清理，再异步加载 catalog/schema，更新 editor holder。运行前要求 server 选中 tag 有效，空 SQL 直接返回；执行目标优先取有效 schema tag，再取启用的 catalog tag，最后 server tag。执行后台 block 使用捕获的目标和执行参数更新该文档 form，然后调用 `runQuery`/`explainQuery`，并非重新从侧栏读取全局目标。`runAllSQL`（`0x10070000c`）复制全文；`runSelectedSQL`（`0x100700108`）复制选区并记录选区起点，两者都汇合到 `runSQL:explain:`。

这些结论证明的是浏览上下文、文档身份和每文档执行目标的静态模型。每文档持有 holder/server 不等于已证明每文档各有独立网络 socket；底层连接池/会话所有权、取消、完整事务与断网后的提交结果仍未知。全部节点枚举、各厂商特殊分支、Lite 实际启用状态和运行中的派发 override 也未覆盖。
