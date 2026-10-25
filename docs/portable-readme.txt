完整解压后运行 sqlshop.exe（Windows）或 sqlshop（macOS / Linux）。
无需安装 Rust。Oracle 连接另需本机 Oracle Instant Client；数据库备份工具需另行安装。

新建连接后选择 MySQL、PostgreSQL、SQL Server 或 Redis，填写服务器和认证信息。
SQLite 文件与 Oracle 查询可从“工具”菜单进入。
本机配置、查询和工作区保存在原有的用户配置目录；压缩包目录不是数据目录。
查询与草稿可能含敏感 SQL，请按需保护配置目录。

此包未签名，macOS 未公证；Linux 需具备相应图形和系统动态库。
校验压缩包旁的 SHA-256 文件后使用。package.json 记录程序的版本、target 与 SHA-256。
第三方组件及其许可见 THIRD-PARTY-NOTICES.tsv 和 licenses 目录。
