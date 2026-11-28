# RPanel

纯 Rust + GPUI 编写的 **Windows / macOS** 轻量级桌面悬浮效率工具。常驻系统托盘，无 WebView 依赖，通过长按右键或全局快捷键随时唤出，自动识别选中文本并展示智能洞察卡片或结构化 JSON 编辑器。

## 核心功能

- **多方式随时唤出**
  - **右键长按**：任意应用中长按右键（默认 `300 ms`，托盘可切换 `250 / 300 / 450 / 600 ms`）在光标旁弹出快捷菜单；短按与拖拽原样放行。
  - **全局快捷键**：`Ctrl+Alt+Space`（Windows）/ `⌃⌥Space`（macOS）；未选中文本时自动检测剪贴板或恢复上次编辑草稿。
  - **自动取词与剪贴板保护**：优先无感读取选区，必要时安全备份并还原剪贴板；支持终端环境取词与单实例常驻运行。
- **极客智能洞察卡片**
  - 纯本地零网络识别常见开发文本，点击即可复制结果或一键展开为 JSON：
  - Unix 时间戳 / 日期时间、JWT、色值（HEX / RGB）、IPv4 CIDR、Cron 表达式、进制数值、Unicode 转义、URL 编码、Base64 / Base64URL。
- **智能 JSON 化与悬浮编辑器**
  - **5 级启发式解析**：标准 JSON / 宽松 JSON5（单引号、尾逗号、注释、Python 字典及双重转义解包）、TSV / CSV 表格、键值对 / HTTP Headers / `.env`、URL / QueryString、纯文本兜底。
  - **轻量原生编辑体验**：纯 Rust 语法高亮、行号、可视化折叠箭头与一键折叠/展开、格式化 / 压缩 / 转义 / 去转义、实时语法校验与错误行列定位。
  - **置顶与多开对比**：支持固定（Pin）面板防止点击外部关闭，并可同时打开多个面板并排对比。

## 快捷键

| 场景 | Windows | macOS | 说明 |
| --- | --- | --- | --- |
| 全局 | `Ctrl+Alt+Space` | `⌃⌥Space` | 唤出面板（剪贴板 / 草稿恢复） |
| 快捷菜单 | `Enter` / `Ctrl+C` / `Esc` | `Enter` / `⌘C` / `Esc` | 打开编辑器 / 复制洞察结果 / 关闭 |
| 编辑器 | `Ctrl+Enter` | `⌘↵` | 复制并关闭（固定状态下仅复制） |
| 编辑器 | `Ctrl+E` / `Ctrl+M` | `⌘E` / `⌘M` | 格式化 / 压缩为单行 |
| 编辑器 | `Ctrl+Shift+[` / `]` | `⌘⌥[` / `]` | 折叠节点 / 全部展开 |
| 编辑器 | `Ctrl+P` / `Ctrl+N` | `⌘P` / `⌘N` | 固定面板 / 新建对比面板 |
| 编辑器 | `Esc` | `Esc` | 关闭面板（未固定时点击外部自动隐藏） |

## 项目结构

```text
RPanel/
├── crates/
│   ├── json-engine/   # 智能解析、格式化、校验与极客洞察检测
│   ├── input-hook/    # 跨平台右键长按拦截、全局快捷键、取词与单实例保护
│   └── app/           # GPUI 快捷菜单、悬浮 JSON 编辑器与系统托盘
└── scripts/
    ├── build-macos.sh     # macOS RPanel.app 打包脚本
    └── build-windows.ps1  # Windows x64 单文件打包脚本
```

## 运行与构建

### 本地运行与调试

```bash
# 常驻系统托盘运行
cargo run --locked -p rpanel

# 预览悬浮编辑器 / 快捷菜单
cargo run --locked -p rpanel -- --preview
cargo run --locked -p rpanel -- --preview-menu

# 单独调试右键长按手感与取词
cargo run --locked -p input-hook --bin hook-cli -- --hold-ms 300 --move-px 8
```

> **macOS 提示**：首次启用右键长按与自动取词时，需在「系统设置 → 隐私与安全性 → 辅助功能」中授予 RPanel 权限。

### 测试与打包

```bash
# 单元测试与 Clippy 检查
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings

# macOS 打包 -> dist/RPanel.app
./scripts/build-macos.sh
```

Windows（需安装 Rust 与 MSVC C++ Build Tools）在 PowerShell 中执行：

```powershell
.\scripts\build-windows.ps1
```

产物输出至 `target/x86_64-pc-windows-msvc/release/rpanel.exe` 与 `dist/rpanel-windows-x64.zip`。
