# RPanel (GPUI Edition)

RPanel 是一款纯 Rust 编写、支持 **Windows 与 macOS 双端** 的轻量级桌面效率工具：全局长按鼠标右键弹出悬浮面板，自动捕获当前选中文本并智能 JSON 化，提供 Tree-sitter 语法高亮、行号、实时校验与一键格式化/复制体验。

## 核心特性

- **全局右键长按拦截（双端原生）**
  - 按住右键超过阈值（默认 `300 ms`，可在托盘菜单切换 `250 / 300 / 450 / 600 ms`）且鼠标位移不超过 `8 px` 时，吞掉系统默认右键菜单并在光标附近弹出浮窗。
  - 短按右键与右键拖拽自动补发原生事件，所有注入事件均携带 `RPANEL_INJECTED_TAG` 防递归标记，不影响正常操作。
  - **Windows**：`SetWindowsHookExW(WH_MOUSE_LL)` 独立消息循环线程 + 多屏虚拟桌面归一化 `SendInput` 补发 + HiDPI 逻辑坐标换算。
  - **macOS**：`CGEventTap` 拦截 + `TapDisabledByTimeout` 自动重启用 + `AXIsProcessTrustedWithOptions` 辅助功能权限检测与引导。
- **自动取词与剪贴板保护**
  - 通用路径：`arboard` 备份原剪贴板 → 模拟 `Ctrl+C` / `Cmd+C` → 读取选中文本 → 自动恢复原剪贴板。
  - macOS 优先尝试通过 `AXUIElement` 无感读取选中文本，不支持时无缝回退至 `Cmd+C`。
- **智能 JSON 化引擎（5 级启发式解析链）**
  1. 标准 JSON / 宽松 JSON5（单引号、尾逗号、注释、无引号键名、Python 字典及双重转义 JSON 字符串解包）
  2. 表格文本（TSV / CSV → JSON 对象数组，保留前导零编码）
  3. 键值对 / HTTP Headers / `.env` / 类 YAML（支持重复键自动数组化）
  4. URL / QueryString（支持完整 URL 与裸查询串、URL 解码与重复参数数组化）
  5. 纯文本兜底（包装为可编辑的 `{"text": "..."}`）
- **纯 Rust GPUI 悬浮面板与系统托盘**
  - 基于 `gpui-kit`（`gpui` + `gpui-component`）构建无 WebView 的轻量原生界面，克制浅色 Apple 风格。
  - 内置纯 Rust JSON 语法高亮器（无 `tree-sitter` C 库依赖，避免 Windows `ts_query_cursor_next_match` 崩溃）、行号、代码折叠、撤销栈保留的格式化/压缩/转义/去转义、内联错误下划线与点击状态栏跳转错误行列。
  - 集成 `tray-icon` 系统托盘：从剪贴板打开、启用/暂停拦截、切换长按时长阈值、退出程序。

## Workspace 架构

```text
RPanel/
├── Cargo.toml               # Workspace 根配置
├── crates/
│   ├── json-engine/         # 纯逻辑智能 JSON 化、格式化、转义与校验引擎
│   ├── input-hook/          # 跨平台右键长按状态机、双端 Hook、取词与 hook-cli 调试入口
│   └── app/                 # GPUI + gpui-component 悬浮编辑面板与系统托盘
└── scripts/
    ├── build-windows.ps1    # Windows x64 单文件 Release 打包脚本
    └── build-macos.sh       # macOS RPanel.app 打包脚本
```

## 快速运行与调试

### 1. 运行桌面主程序
```bash
# 常驻系统托盘运行
cargo run --locked -p rpanel

# 立即弹出预览面板检查界面（默认 300ms 阈值，可用 --hold-ms 调整）
cargo run --locked -p rpanel -- --preview --hold-ms 300
```

### 2. 单独调试右键长按手感与取词（CLI）
```bash
cargo run --locked -p input-hook --bin hook-cli -- --hold-ms 300 --move-px 8
```

### 3. 面板快捷键
- `Esc`：隐藏浮窗（点击浮窗外部失焦也会自动隐藏）
- `Ctrl+Enter`（Windows） / `Cmd+Enter`（macOS）：一键复制并隐藏
- `Ctrl+E` / `Cmd+E`：美化格式化（Pretty）
- `Ctrl+M` / `Cmd+M`：压缩为单行（Minify）

## 测试与打包

```bash
# 运行全部单元测试与 Clippy
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings

# macOS 打包 dist/RPanel.app
chmod +x scripts/build-macos.sh
./scripts/build-macos.sh
```

在 Windows PowerShell（已安装 Rust 与 MSVC C++ Build Tools）中打包单文件 Release：
```powershell
.\scripts\build-windows.ps1
```
输出文件：`target/x86_64-pc-windows-msvc/release/rpanel.exe` 与 `dist/rpanel-windows-x64.zip`。
