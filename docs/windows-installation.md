# Windows 用户安装与文件打开方式

SQLShop、LinkShop、RPShop 使用相同的当前用户安装流程。需要 Windows、Python 3.9+ 和与本机架构一致的 Windows MSVC 包。安装不需要管理员权限。

## 从仓库打包并安装

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1 -Install -OutputDirectory .\dist\installed-candidate
```

`-Install` 执行首次安装，创建开始菜单 `Shop` 文件夹下的产品快捷方式，并注册支持的文件类型。已有安装属于当前仓库时，后续普通的成功打包会自动更新安装：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1 -OutputDirectory .\dist\next-candidate
```

同版本重复打包需指定新的输出目录，原有发布包不覆盖。只打包、不更新安装时加 `-NoInstall`；Python 对应参数为 `--install`、`--no-install`。自动更新仅适用于本机 Windows、匹配架构且成功完成的包；CI、跨平台构建和另一个仓库检出不会自动更改该安装。构建失败不更新开始菜单入口或当前版本指针。

已有完整包也可单独安装：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1 -PackageDirectory 'D:\Packages\SQLShop-0.1.0-windows-x64'
```

从解压包内执行 `install.ps1` 时可省略 `-PackageDirectory`。仓库中的入口会记录源仓库位置，供以后成功打包自动刷新；独立解压包安装没有源仓库，后续需再次安装新的包或在仓库中显式执行 `-Install`。

安装需要当前用户的桌面 Explorer 正在运行。入口通过 Explorer 的桌面自动化对象启动隐藏的 Python 代理，在真实用户空间读取安装状态、写入开始菜单与 HKCU；这样从 Codex 等 MSIX 工具启动时也不会只写入工具私有的 AppData/注册表视图。代理使用目录句柄检查实际开始菜单路径，确认没有重定向才执行；父调用等待代理完成并保留错误输出，不提前报告成功。不需要管理员或任务计划，不修改工具包 manifest。临时通信目录位于用户主目录而非 AppData，完成后自动清理；超时则保留目录供诊断。[Microsoft 的 Explorer 启动示例](https://devblogs.microsoft.com/oldnewthing/20131118-00/?p=2643/) 与 [MSIX 虚拟化说明](https://learn.microsoft.com/en-us/windows/msix/desktop/flexible-virtualization) 解释了这两个上下文的区别。

## 稳定快捷方式与不可变版本

```text
%LOCALAPPDATA%\Programs\Shop\<Product>\
  <Product>.exe                 稳定的无控制台启动器
  current.path                  最新完整版本的绝对路径，UTF-8
  install-state.json            源仓库、版本、校验和和安装位置
  versions\<version>-<exe-sha>-<package-sha>\
    <crate-name>.exe
    package.json
    THIRD-PARTY-NOTICES.tsv
    licenses\ ...
%APPDATA%\Microsoft\Windows\Start Menu\Programs\Shop\<Product>.lnk
```

`<Product>` 为 `SQLShop`、`LinkShop` 或 `RPShop`。开始菜单快捷方式和注册表中的打开命令都指向稳定启动器，而非某次打包的 `dist` 路径。安装先校验程序、启动器和复制后的整个包，再原子切换 `current.path`；源 `dist` 包以后可以移动。快捷方式图标来自当前版本的程序。

每次启动都解析最新指针，并将原始 Windows 参数转交给对应程序，支持空格和中文路径。已运行的实例保留当前会话；关闭后重新打开才使用新版本。安装器不会强杀进程、替换正在运行的程序或删除旧版本目录，因此不影响未保存的工作。旧目录暂不自动清理。

三个仓库共用相同的 `src/shell_open.rs`。Windows 下，同一用户登录会话中的同一产品只保留一个 GUI 实例；从开始菜单或文件再次启动会向现有窗口转发请求并恢复、聚焦窗口。SQLShop 只载入 SQL 文件供编辑，不自动执行；SQLite 使用已有本地数据库入口。LinkShop 调用已有 Postman collection / environment 导入。RPShop 打开文件前保留现有未保存确认流程。纯命令行功能继续独立运行，不占用 GUI 单实例入口。

## 文件类型与默认应用

| 工具 | 注册到“打开方式”的类型 |
| --- | --- |
| SQLShop | `.sql`、`.sqlite`、`.sqlite3`、`.db`、`.db3` |
| LinkShop | `.json`，用于 Postman collection / environment |
| RPShop | `.rpshop`、`.rp`、`.rplib`、`.json` |

`.json`、`.db` 等通用扩展名可能属于其他工具；文件仍需符合所选应用实际支持的格式。脚本在 HKCU 注册 `App Paths`、应用/ProgID 打开命令、`OpenWithProgids`、`SupportedTypes` 和 `Capabilities` / `RegisteredApplications`，保留现有扩展名默认值及受保护的 `UserChoice`。注册本身不会静默切换默认应用。

在文件的右键菜单中选择“打开方式”，或在 Windows 设置的“默认应用”中选择对应工具。安装时加 `-OpenDefaultApps` 可以打开产品的默认应用设置页；也可使用官方设置地址：

```text
ms-settings:defaultapps?registeredAppUser=SQLShop
ms-settings:defaultapps?registeredAppUser=LinkShop
ms-settings:defaultapps?registeredAppUser=RPShop
```

这些名称就是 HKCU `Software\RegisteredApplications` 中登记的名称。[Microsoft 的默认应用设置文档](https://learn.microsoft.com/en-us/windows/apps/develop/launch/launch-default-apps-settings) 说明了 `registeredAppUser` 参数及适用的 Windows 11 版本；其他版本仍可在“默认应用”中搜索产品名称。安装与打包脚本都不会绕过系统默认应用确认流程。

## 验证

```powershell
python -m unittest discover -s scripts/tests -v
```

默认测试使用临时目录和注册表/快捷方式替身，不改变用户安装。覆盖安装更新、旧版本保留、校验失败、注册失败、自动更新范围，以及保留已有默认应用的注册契约。

额外验证真实 Explorer 代理可设置 `SHOP_TEST_DESKTOP_BROKER=1` 后运行 `test_windows_desktop.py`。此测试只运行临时参数记录程序，验证中文、空格路径、工作目录、错误输出和非零退出码，不执行安装或修改 AppData。

可在开发工具链就绪后额外运行真实启动器测试；该测试只启动临时生成的参数记录程序，不操作产品窗口：

```powershell
. .\scripts\common.ps1
Initialize-ShopToolchain
rustc --edition 2024 --crate-name shop_launcher -C target-feature=+crt-static scripts/windows/launcher.rs -o target/shop-launcher-test.exe
$env:SHOP_TEST_LAUNCHER_EXE = (Resolve-Path target/shop-launcher-test.exe).Path
python -m unittest discover -s scripts/tests -p test_windows_launcher.py -v
Remove-Item Env:\SHOP_TEST_LAUNCHER_EXE
```
