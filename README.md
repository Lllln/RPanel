# RPanel

RPanel 是面向 Windows 10 / 11 的 Rust 桌面工具，阶段 2 也提供 macOS 交互原型。目标交互是选中文本、松开长按的右键、在光标附近点击「查看Json」。程序自动复制选中的原文，同时打开可编辑的 JSON 窗口。窗口计划支持格式化显示、对象和数组的折叠与展开、语法错误定位、重新格式化和压缩复制。

## 阶段 2 交互原型

在装有 Rust 和 MSVC C++ 构建工具的 Windows x64 环境运行 `cargo run --locked`。原型启动后驻留在系统托盘。对前台窗口长按右键至少 500 ms 后松开，满足手势条件时会看到浅色「查看Json」按钮。点击按钮只关闭原型面板并写一条日志。右键托盘图标可停用或重新启用「监听右键长按」，以及退出程序。运行 `cargo run --locked -- --hold-ms 700` 可用 700 ms 取代默认阈值。

原型放行右键事件。长按时允许同一应用的原生上下文菜单与 RPanel 面板同时出现，原生菜单可能先接收点击。进入系统菜单、正在移动窗口、处于全屏或提权应用、前台目标切换时跳过弹出。选区读取、自动复制、编辑窗口和备用热键由后续阶段实现。

从托盘选「预览『查看Json』」，或运行 `cargo run --locked -- --preview`，可以单独检查面板的显示与点击。若预览能显示而长按无效，可关闭旧实例，在 PowerShell 中设置 `$env:RUST_LOG = 'rpanel=debug'` 后运行 `cargo run --locked`，查看是否检测到了长按以及窗口检查结果。

本仓库通过 `rust-toolchain.toml` 锁定 Rust 1.100.0。macOS arm64 已通过 `cargo fmt --check`、`cargo test --locked` 的 7 项测试和 Clippy。安装 `x86_64-pc-windows-msvc` 标准库后，该目标的 `cargo check` 和 Clippy 也已通过。这些检查覆盖阈值、取消、时间戳回绕和工作区定位。Windows 可执行文件链接与桌面交互仍待验证。

Windows 实机先对照关闭和启用监听时的短按、双击及拖动行为，再分别检查阈值内外、原生菜单冲突、长按松开后焦点归属、菜单点击和外部点击。最后覆盖托盘切换与退出、触控板、混合 DPI 和外接屏负坐标；记录系统与应用版本。

### Windows 编译与打包

在装有 Rust 和 MSVC C++ Build Tools 的 Windows PowerShell 中，从项目根目录执行所需命令。

```powershell
cargo run --locked                                  # 编译并运行调试版
cargo build --locked                                # 只编译调试版
cargo build --release --locked --target x86_64-pc-windows-msvc  # 只编译 Release
.\scripts\build-windows.ps1                         # 编译并打包 Windows x64 Release
```

打包脚本仅在构建时启用静态 C 运行库链接。生成的单文件位于 `target/x86_64-pc-windows-msvc/release/rpanel.exe`，`dist/rpanel-windows-x64.zip` 中仅含这一个文件。发布前仍需在干净的 Windows 上验证运行库依赖和双击启动。[Rust CRT 链接说明](https://doc.rust-lang.org/stable/reference/linkage.html#static-and-dynamic-c-runtimes)

在 Windows 上双击 Release 版时，程序只在系统托盘驻留，长按右键后才显示面板。调试版使用控制台输出日志。

### 在 macOS 测试

在 macOS 桌面运行 `cargo run --locked`，从菜单栏的 RPanel 图标打开「预览『查看Json』」可以直接测试面板。也可运行 `cargo run --locked -- --preview` 在启动时预览，或在其他应用里按住鼠标右键至少 500 ms 后松开。菜单栏图标提供监听开关和退出。点击面板里的「查看Json」会关闭面板并写日志，本阶段未接入选区、剪贴板或 JSON 编辑。

- macOS 使用 AppKit 的全局与本地鼠标观察器，在主线程处理鼠标事件。全局观察器不改变事件，本地观察器原样返回事件。当前只订阅鼠标事件，原型无需申请「辅助功能」或「输入监控」，也不会模拟输入。[Apple 事件监听说明](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/MonitoringEvents/MonitoringEvents.html)
- 面板使用 `orderFrontRegardless` 显示，显示时不主动激活应用。点入面板后的焦点归属仍受 AppKit 窗口行为影响。定位使用 `NSScreen.visibleFrame` 与逻辑点，避开菜单栏和 Dock。原生菜单仍可能同时出现，触控板请启用「辅助点按」并用能产生右键按下、松开事件的手势。全屏、不同桌面空间和外接屏留待进一步试验。[Apple 窗口显示接口](https://developer.apple.com/documentation/appkit/nswindow/orderfrontregardless%28%29) · [屏幕可视区域](https://developer.apple.com/documentation/appkit/nsscreen/visibleframe)
- 如果后续在 Mac 接入跨应用选区读取，需要另行验证对方控件是否暴露选区，以及系统的辅助功能授权。Windows 的 UI Automation 验收仍在 Windows 完成。

macOS arm64 已运行预览，确认浅色边框和中文标签正常显示，点击面板后关闭且记录触发日志。TextEdit 的原生右键菜单仍会出现。实体鼠标和触控板的跨应用长按、应用间焦点及多屏定位仍需实测。

## 右键长按与原应用行为

阶段 2 先试验**只观察的 `WH_MOUSE_LL`**。按下时记录 `MSLLHOOKSTRUCT` 中的时间戳和屏幕坐标，移动超过容差、发生冲突输入或监听关闭时取消候选。松开时使用可处理计时回绕的时间差判断是否达到本次按下时记录的阈值，默认 500 ms，达到阈值且候选仍有效才发出一次触发事件。移动容差按按下位置的显示器 DPI 换算，初值在原型中实测。未配对的松开、注入事件和失效候选全部放行。前台窗口上下文在手势期间只记录轻量标识，菜单显示前再确认目标窗口与鼠标命中窗口归属。[LowLevelMouseProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc) · [MSLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msllhookstruct)

| 方案 | 行为与取舍 |
| --- | --- |
| 只观察 `WH_MOUSE_LL` | 按下和松开均通过 `CallNextHookEx` 传递，保留短按、双击及右键拖动的原始输入。能在事件发生时取得坐标和时间，也可观察菜单外点击。回调超时可能导致系统静默卸载 Hook。 |
| 只观察 Raw Input | 用 `RIDEV_INPUTSINK` 接收后台 `WM_INPUT`，避免低级 Hook 的回调时限。需要验证触控板合成右键、左右键交换和多鼠标的实际行为，并另取光标坐标。保留为被动监听备选。 |
| 拦截或重放 | 按下时无法预知用户是否会长按。先拦截再重放会改变短按时序、焦点和鼠标捕获，且 `SendInput` 受 UIPI 限制。只吞掉松开则可能使原应用收不到配对事件。首版排除此方案。 |

[Microsoft 对低级 Hook 的建议](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc) · [Raw Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/using-raw-input) · [SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)

**原应用可能同时响应长按或弹出自己的右键菜单。** 原型用 `GetGUIThreadInfo` 检查移动窗口、系统菜单和当前应用的上下文菜单。显示面板时允许命中同线程的原生上下文菜单窗口。原生菜单有可能先接收点击，可先按 Esc 关闭原应用菜单。支持全局停用，按应用排除属于后续计划。遇到可识别的独占全屏、权限边界或目标不明时跳过触发。首版按普通用户权限运行，游戏、管理员窗口、UAC 安全桌面和受保护应用不列为自动集成支持范围。全屏和应用识别属于尽力而为，特殊应用可关闭监听。[GetGUIThreadInfo](https://learn.microsoft.com/en-us/windows/desktop/api/winuser/nf-winuser-getguithreadinfo) · [GUITHREADINFO](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-guithreadinfo) · [RegisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey) · [UI Automation 权限说明](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview)

## 悬浮菜单、焦点与多显示器

egui 负责绘制和交互，eframe 负责宿主窗口及渲染。`ViewportBuilder::with_active(false)` 只设置初始焦点。原型优先把 eframe 主 viewport 用作可复用的菜单窗口，从 `CreationContext` 通过 `HasWindowHandle` 取得 HWND，初始隐藏后配置 `WS_EX_NOACTIVATE` 和 `WS_EX_TOOLWINDOW`。由 HWND 所属线程使用 `SetWindowPos(HWND_TOPMOST, …, SWP_NOACTIVATE | SWP_SHOWWINDOW)` 定位并显示。若实测点击仍抢焦点，在同一线程处理 `WM_MOUSEACTIVATE` 并返回保留点击的 `MA_NOACTIVATE`。原型要验证点击、悬停以及 Windows 开启「将鼠标悬停在窗口上来激活窗口」时的行为，同时检查 eframe 是否仍能正常接收按钮事件。[egui ViewportBuilder](https://docs.rs/egui/latest/egui/viewport/struct.ViewportBuilder.html) · [eframe CreationContext](https://docs.rs/eframe/latest/eframe/struct.CreationContext.html) · [扩展窗口样式](https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles) · [窗口激活](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-mouse-input#window-activation) · [SetWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos)

- 以 Per-Monitor DPI Awareness V2 为目标，使用光标所在显示器的 `rcWork`。将 egui 点数换算成该屏的物理像素后再计算外框位置，先尝试光标右下方，空间不足时翻转并限制在工作区内。覆盖负坐标、不同任务栏位置、混合缩放和屏幕切换。
- 菜单显示期间才判断屏幕坐标中的外部按下事件，点击继续交给原应用。监听线程只通知 GUI 关闭，坐标和菜单代次随事件传递，避免旧事件关闭新菜单。菜单本身没有键盘焦点，键盘操作从托盘或热键进入可聚焦窗口。
- JSON 编辑窗口和设置使用普通可聚焦的独立 viewport。点击菜单后隐藏面板，打开编辑窗口，关闭编辑窗口后返回托盘驻留。若实测 eframe 宿主无法同时满足不激活和命中，先保留已保存选区之后才激活的菜单作为显式降级，由用户选择是否启用。托盘可打开空白编辑窗口供手动粘贴，热键可触发同一面板。

[Windows 高 DPI 桌面开发](https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows)

### 视觉方向

默认采用 Apple-inspired 的浅色风格。用留白、字重、细边框和轻阴影区分层级。编辑窗口用雾灰色作窗口底，输入区和悬浮菜单用不透明白色，蓝色只强调焦点和主要反馈。原型起始色为背景 `#F5F5F7`、内容与浮层 `#FFFFFF`、细边框 `#DDE0E5`、正文 `#1D1D1F`、次级文字 `#60646C`、强调色 `#005CB9`，再根据 Windows 实际显示效果调整。界面使用清晰的无衬线中文字体，JSON 内容使用等宽字体并为中文字符配置回退。

- **悬浮菜单**只呈现「查看Json」一行。使用约 12 点圆角、8 点内边距和 36 点高的整行点击区。悬停以低对比底色变化提示。首轮用实色验证观感与响应，窗口显示和点击不依赖模糊效果或入场动画。
- **JSON 编辑窗口**沿用浅色配色，主体使用可滚动的白底等宽文本编辑区。窗口底部固定工具栏，编辑区独立滚动。工具栏右侧提供「格式化」和「压缩复制」，可对当前内容反复执行。左侧显示校验与复制状态，错误位置可点击跳转。JSON 树形查看留待后续版本。
- **语法配色**在同一编辑区呈现。键名用紫色 `#7440A0`，字符串用绿色 `#23744C`，数字用棕橙色 `#97501B`，布尔值与 `null` 用蓝色 `#2255A4`，括号、逗号和冒号用灰色 `#626971`。错误位置用红色下划线 `#B42318`，并配合行列文字。编辑中即使暂时出现无效 JSON，已识别的内容仍保留配色，光标和选区随正常编辑操作更新。
- **视觉验收**在 Windows 100%、125%、150% 和 200% 缩放下检查中文与等宽文本的清晰度、浅底上的类型色对比度、点击区域、内容截断和焦点状态。用真实 JSON 样本检查长键名、深层嵌套和窗口缩放。在明暗背景上反复弹出菜单，检查边界是否清晰、窗口是否闪烁。

## 选区读取与剪贴板

1. 合格的右键松开或选区快捷键就是一次主动请求。显示菜单前保存来源窗口 HWND、进程与线程标识及可取得的焦点 HWND，由 UI Automation 工作线程立即读取一次。按住右键期间只判断手势。用户点击菜单时复用本次结果，仍在读取则只显示短暂等待状态。
2. 首选 `IUIAutomation::GetFocusedElement`，校验元素属于触发时的窗口上下文。焦点元素没有可用 pattern 时，可尝试保存的焦点 HWND 或触发点，并仅在同一窗口上下文内检查有限层数的祖先。元素的进程可与宿主 HWND 不同，浏览器场景需要实测辨认归属。焦点变更、目标退出或归属不明确时放弃读取，避免使用另一个窗口的文本。
3. 通过 `UIA_TextPatternId` 取得 `IUIAutomationTextPattern`，调用 `GetSelection` 后对范围调用带长度上限的 `GetText`。空范围表示没有有效选区，多个非空范围提示改为单选或手动粘贴。超限时提示用户缩小选区。尊重密码和受保护控件，避免读取整个编辑框或文档当作选区。右键按下或原生菜单可能已改变选区，此时不保证恢复长按前的内容。
4. 点击「查看Json」后，已取得的完整选区原文直接写入剪贴板一次，编辑窗口和解析流程独立继续。剪贴板成功或失败按实际结果反馈。无选区、控件不支持、目标失效、超时或超限时打开编辑窗口供手动粘贴，等待用户提供完整文本。后台不订阅选区变化，也不模拟 `Ctrl+C`。

[获取 UI Automation 元素](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-obtainingelements) · [GetSelection](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationtextpattern-getselection) · [GetText](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationtextrange-gettext) · [剪贴板所有权与格式](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-operations)

阶段 3 按**控件**记录支持结果。记事本测 Windows 10 / 11 各自版本的纯文本编辑区。VS Code 测编辑器、集成终端和无障碍模式 `auto` / `on`。Chrome / Edge 分别测网页正文、`textarea` / `contenteditable`、地址栏、开发者工具与内置 PDF。记录系统与应用版本、触发方式、选区是否仍在，以及自动读取或手动粘贴的实际结果。[VS Code 无障碍模式](https://code.visualstudio.com/docs/configure/accessibility/accessibility)

## JSON 查看与编辑

- **首次打开**立即展示可编辑文本。在预算内能安全格式化时，编辑区显示缩进后的内容。解析和换行修复均失败时保留原文，显示第一处错误的行、列和附近片段，用户可跳转到对应位置。自动格式化结果只在编辑区还处于初始版本时应用，避免覆盖提前开始的编辑。
- **折叠和展开**在编辑区左侧提供控件，可逐个收起或展开多行对象和数组，包括嵌套节点。折叠只隐藏显示，完整文本仍用于编辑器操作、格式化和压缩复制。跳转到折叠区内的错误时自动展开对应范围。编辑导致括号范围失效时，展开受影响的部分并重新计算折叠范围。
- **格式化**以当前编辑内容为准，成功后更新编辑区。校验和换行修复均失败时保留编辑内容，并更新错误位置。普通编辑和格式化均无需写入剪贴板。
- **换行处理**在首次打开和点击「格式化」时先按标准 JSON 解析。对象、数组等结构之间的 LF 和 CRLF 按正常空白处理。若解析失败，且能明确识别出双引号字符串中的真实换行，则尝试把 LF、CR 或 CRLF 分别写成 `\n`、`\r` 或 `\r\n`，保留字符串中的换行含义。完整重验通过后才应用修复，并提示「已转义字符串中的换行」。遇到引号不闭合、断在数字中的换行或多段独立 JSON 等情况，保留当前文本并显示原始错误位置。打开面板时已复制的原文仍保留原始换行。[RFC 8259 空白规则](https://www.rfc-editor.org/rfc/rfc8259#section-2) · [字符串规则](https://www.rfc-editor.org/rfc/rfc8259#section-7)
- **压缩复制**以当前编辑内容为准，校验通过后把去掉无意义空白的单行 JSON 写入剪贴板。编辑区继续显示当前文本。输入无效时显示错误位置并保留剪贴板。格式化和压缩保留键顺序、重复键、数字字面量及字符串内容。

## 线程与资源所有权

| 线程 | 职责 |
| --- | --- |
| GUI 主线程 | 运行一次 eframe/winit 事件循环，管理菜单、编辑窗口、设置和托盘动作的应用状态。收到应用事件后按需唤醒并重绘，无空闲轮询。 |
| Windows 监听线程 | 独占 Hook 和阻塞式 Win32 消息循环，处理监听开关及备用 `WM_HOTKEY`。Hook 回调只更新轻量状态并保存必要的事件数据，避免日志、分配密集操作、阻塞等待和跨进程调用。回调投递线程消息后立刻放行，消息循环再投递有界应用事件并按需唤醒 GUI。禁用时清空候选并卸载 Hook。 |
| UI Automation 工作线程 | 无窗口的 COM MTA 线程，持有并释放 UIA 接口。请求设置单次连接和事务超时，并独立设置界面等待期限。单次取选区涉及多次 COM 调用，界面超时不等于调用已取消。至多一个调用任务在途，过期结果按请求编号丢弃，避免慢 provider 导致队列或线程增长。 |
| JSON 工作线程 | 阶段 4 引入，用有界队列处理校验、格式化和压缩，按输入版本返回结果或错误位置。首次复制原文与后台解析独立。「压缩复制」只使用本次动作对应的校验结果。 |

线程间只传递值和应用事件，COM 对象及窗口操作留在所属线程。退出时停止接收请求，撤销热键与 Hook，移除托盘图标，再关闭窗口和工作线程。测试慢 provider 的退出路径，确保 GUI 不因等待 COM 而卡住。FFI 封装在 `platform::windows`，每个 `unsafe` 说明指针有效期、句柄归属和回调约束。日志只记录阶段、错误码和耗时，选区及 JSON 内容留在内存，取消操作或关闭编辑窗口时按生命周期释放。[UI Automation 线程模型](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading) · [UI Automation 事务超时](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation2-put_transactiontimeout) · [egui 请求重绘](https://docs.rs/egui/latest/egui/struct.Context.html#method.request_repaint)

## 项目结构与依赖

```text
RPanel/
├── Cargo.toml
├── Cargo.lock
├── README.md
└── src/
    ├── main.rs
    ├── lib.rs
    ├── app.rs
    ├── core/
    │   ├── mod.rs
    │   └── json.rs
    ├── platform/
    │   ├── mod.rs
    │   ├── windows/
    │   │   ├── mod.rs
    │   │   ├── mouse.rs
    │   │   ├── popup.rs
    │   │   ├── selection.rs
    │   │   └── clipboard.rs
    │   └── macos/
    │       ├── mod.rs
    │       ├── mouse.rs
    │       └── popup.rs
    └── ui/
        ├── mod.rs
        ├── panel.rs
        ├── editor.rs
        └── settings.rs
```

单 Cargo 包按阶段增添文件。`main` 启动，`app` 管理设置和事件并组合 `core`、`platform`、`ui`。`core` 只处理 JSON 与可单测的数据逻辑。`platform` 提供手势、选区、窗口、托盘和剪贴板操作。`ui` 接收状态并返回用户动作。`platform/windows` 与 `platform/macos` 各自处理原生鼠标和窗口行为，应用状态、面板绘制、手势判定与定位算法共用。`cargo test --lib` 可单独运行平台无关测试。

| 依赖 | 选择 |
| --- | --- |
| Rust Stable / Edition 2024 | 阶段 2 在 Windows 构建机确定所选依赖要求的最小 Rust 版本，并提交 `Cargo.lock`。 |
| `eframe` / `egui` | 同一版本系列，阶段 2 使用 `glow`，Windows 的启动和隐藏恢复待实机验证。原型从 Windows 字体目录加载中文字体回退，正式版需验证干净系统中的字体覆盖。 |
| `windows` / `raw-window-handle` | Windows 专属，按实际使用启用 Win32 功能。前者用于鼠标、COM/UIA、窗口、托盘和剪贴板，后者用于提取 eframe 窗口句柄。 |
| `serde_json` | 解析和格式化，保留默认递归保护。验证键顺序、重复键和高精度数字的语义后再定格式化策略，避免把可能改写值的输出标成无损格式化。 |
| `objc2` / `objc2-app-kit` / `objc2-foundation` / `block2` / `winit` | macOS 专用。使用类型化 AppKit 绑定注册鼠标监听，读取显示器工作区，控制 eframe 的原生窗口与应用激活策略。 |
| `tray-icon` | 复用 Windows 系统托盘和 macOS 菜单栏状态项，面板隐藏期间可切换监听和退出。 |
| `log` / `env_logger` | 记录关键状态和错误，阶段 2 输出到运行原型的终端，发布版日志位置在集成阶段定稿。跨线程通信用标准库，首版无需异步运行时。 |

[eframe 原生配置](https://docs.rs/eframe/latest/eframe/struct.NativeOptions.html) · [serde_json 反序列化接口](https://docs.rs/serde_json/latest/serde_json/struct.Deserializer.html)

## 实施与验收

| 阶段 | 实施内容 | 验收条件 |
| --- | --- | --- |
| 2 · 交互原型 | 建单包，先做时间与移动判定、只观察 Hook、监听开关和「查看Json」单项占位菜单，再验证不激活的显示和关闭。 | Windows 验证阈值两侧、短按、双击、拖走再移回、开关中断、缺失按键事件和高频鼠标。用 Win32 测试窗对照启用前后的输入顺序，再在实际应用测原生菜单共存、焦点、外部点击、多屏与 DPI。检查菜单在各缩放档位的视觉效果，记录触控板或交换键位的差异。 |
| 3 · 选区 | 加入 UIA 工作线程、窗口上下文校验、超时和手动粘贴。 | 按上述控件矩阵实测。覆盖空选区、密码控件、焦点变更、目标退出、连续触发及慢 provider。确认仅主动触发后读取，粘贴降级前原剪贴板内容仍可用。 |
| 4 · JSON 查看与编辑 | 点击后自动复制原文并打开带语法配色的编辑窗口，实现折叠和展开、错误定位、格式化、换行修复和压缩复制。 | 首轮采用 4 MiB UTF-8 格式化预算并保留默认深度保护。验证合法、无效和超预算输入在取得完整原文时均自动复制。测试嵌套折叠、折叠时格式化及压缩、错误跳转展开、字符串内的 LF 和 CRLF、已有转义及拒绝不明确的修复。检查语法配色、Unicode 错误位置、固定工具栏和剪贴板失败反馈。 |
| 5 · 集成与发布 | 完成托盘、配置持久化和正常退出。首先构建 `x86_64-pc-windows-msvc` Release，检查依赖并在未安装 Rust 的干净 Windows 环境启动。 | 覆盖睡眠恢复、反复显示和隐藏、关闭编辑窗口、监听禁用、退出后输入正常及句柄释放。记录 Windows 10 / 11 的待机 CPU、重绘次数、内存与句柄变化，以及持续运行结果。若有额外 VC 运行库依赖，验证静态 CRT 配置与实际产物后再交付独立 exe。 |

从阶段 2 起，每阶段执行适用的 `cargo fmt --check`、`cargo check --target x86_64-pc-windows-msvc`、`cargo test` 和 `cargo clippy --all-targets -- -D warnings`。阶段 5 额外执行 `cargo build --release --locked --target x86_64-pc-windows-msvc`。兼容性与性能结果以 Windows 桌面环境的实测记录为准。[Rust Windows MSVC 目标](https://doc.rust-lang.org/rustc/platform-support/windows-msvc.html) · [Rust CRT 链接](https://doc.rust-lang.org/stable/reference/linkage.html)

## Gitee 镜像

[Mirror to Gitee](.github/workflows/mirror-gitee.yml) 在 GitHub 推送时同步所有分支和标签到 `git@gitee.com:llllnD/rpanel.git`，也支持在 Actions 页面手动运行。同步保留 Gitee 独有的分支和标签，有分叉历史的引用会被 Git 拒绝更新，请先核对两端历史。

1. 在 Gitee 给有目标仓库推送权限的身份配置专用 SSH 公钥。
2. 在 GitHub 仓库 **Settings → Secrets and variables → Actions** 创建 `GITEE_SSH_PRIVATE_KEY` secret，值为对应的私钥。创建 `GITEE_KNOWN_HOSTS` 变量，值为确认可信的 `gitee.com` `known_hosts` 完整记录。可从已建立信任的终端用 `ssh-keygen -F gitee.com` 查找该记录。[GitHub Actions secrets](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets) · [配置变量](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-variables)
3. 在 GitHub 的 **Actions → Mirror to Gitee** 手动运行一次，检查 Gitee 中的分支和标签。
