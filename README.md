# MeowDown

一个用 **Rust + GPUI Kit** 写的所见即所得 Markdown 编辑器：写下的每一个 `#`、`-`、`>` 都会立刻变成排版好的样子，点进任意段落就能原地编辑，字号和行距纹丝不动。

## 现在能做什么

- **就地编辑**：点哪里，光标就落在哪里；只有光标所在的那一个小元素才露出 Markdown 源码，其余标记全部隐藏
- **块的自动转换**：段首输入 `#`、`-`、`>`、`1.`、`- [ ]` 或三个反引号即切换块类型
- **表格**：`Ctrl+Enter` 在下方插入一行，`Ctrl+Shift+Backspace` 删除当前行，单元格间用方向键走动
- **行内元素**：加粗、斜体、删除线、行内代码（JetBrains Mono，字体已内嵌）、行内公式、超链接
- **超链接**：`Ctrl+点击` 打开；网络地址交给浏览器，相对路径在本窗口打开并滑到 `#标题` 的位置，鼠标悬停显示手型
- **侧边栏**：文件树 + 大纲，`Ctrl+Shift+O` 打开整个文件夹
- **图片**：粘贴即入稿，可配置 PicGo 上传图床；单击就地露出 Markdown 源码，**双击**看全屏大图——滚轮缩放、拖动平移、`←` `→` 翻本篇的上下张，`0` 复位、`Esc` 回来，关闭后顺势停在它的 Markdown 源码上
- **自动保存**、深浅色主题、专注模式、源码模式（`Ctrl+/`）
- **文件关联**：一键把 MeowDown 设为 `.md` 的默认应用

菜单里每个命令都标了快捷键，`Ctrl+Shift+/` 可以直接看总览。

## 构建

需要 Rust 1.85+（edition 2024）和 MSVC 工具链。

```bash
cargo build --release
```

产物在 `target\release\meowdown.exe`，单文件即可运行：字体和图标都编进了 exe。

`vendor/` 里是打过补丁的 `gpui-base`（段落里混排两种字族要靠它），通过 `[patch.crates-io]` 接入，不需要额外操作。

## 打包

一条命令，产物只有一个 NSIS 安装包。版本号自己定，脚本会写回 `Cargo.toml`：

```powershell
powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0
```

产物：`dist\MeowDown-v0.2.0-setup.exe`（约 8 MB）。它装到当前用户的
`%LOCALAPPDATA%\Programs\MeowDown`，不写 `HKLM`、不弹 UAC；向导里可以选桌面
快捷方式、开始菜单和 `.md` 关联，卸载入口在「设置 → 应用 → 已安装的应用」。

重装或升级时会先查一遍这台电脑是不是已经装过（当前用户 / 机器级的两个注册表
视图都看，并且确认目录里程序还在），装上过就把安装路径自动换成原来的目录，
不会又装出一份新的；想换地方在向导的目录页里改，或者先卸载旧的。

脚本不再产出便携目录和 zip：要绿色版直接 `cargo build --release`，文件在
`target\release\meowdown.exe`。

需要 `makensis`：`winget install NSIS.NSIS` 即可，装在别处就用 `-Makensis <路径>`
指定，项目里的 `tools\nsis\` 也会被自动找到。安装脚本是 `installer.nsi`，也可以
自己手动编译。

只想用当前版本号打包：去掉 `-Version`；二进制已经在手、跳过编译：加 `-SkipBuild`；
加上 `-Register` 会顺手把这份 exe 注册为 `.md` 的打开方式。

## 设为 .md 默认应用

三种方式，效果一样：

```powershell
.\MeowDown.exe --register      # 命令行
.\package.ps1 -Register        # 打包时顺手注册
```

或者打开程序里的「设置 → 系统集成 → 设为 .md 默认应用」。

程序会写好 `HKCU\Software\Classes` 下的 ProgID、`OpenWithProgids`，以及让 MeowDown 出现在「默认应用」列表里的 `Capabilities` 块，全程不需要管理员权限。取消用 `--unregister`。

> Windows 10/11 上真正决定「双击用什么打开」的是 `UserChoice`，这个键由系统保护，只允许用户在设置里改。所以程序能做的是把自己登记好，最后一步请点「设置 → 系统集成 → 去系统设置」，在列表里选一次 MeowDown。

## 命令行

```
meowdown [文件.md]    打开文档（也可以是文件夹）
meowdown --register   设为 .md 默认应用
meowdown --unregister 取消关联
meowdown --version    打印版本号
```

## 字体

内置 JetBrains Mono（6 个字重），遵循 SIL Open Font License，许可证见 `assets/fonts/OFL.txt`。
