# OpenRevo Linux (Wayland / KDE) 移植开发步骤

> 分支：`wayland-kde`（自 `dev@9e2e4ce` 派生）　目标：Arch Linux + KDE Plasma Wayland
> 上游文档依据：[`docs/core/sdk-integration.md`](../core/sdk-integration.md) §1、§5；[`docs/skill.md`](../skill.md) §2
> 作者在 §5 已给出 Linux 路线：HAL 底层走 `ec_sys` + NVML，产物形态 `libopenrevo_core.so`。本文件把该路线拆成可执行步骤，并补齐作者未展开的平台外壳部分。

---

## 0. 现状评估（实测，非推测）

### 0.1 数据

在 `src-tauri/` 执行 `cargo check -p openrevo-core`（rustc 1.100.0，Linux x86_64）：

```
error: could not compile `openrevo-core` (lib) due to 324 previous errors
```

按文件分布：

| 文件 | 错误数 | 性质 |
| :--- | ---: | :--- |
| `crates/openrevo-core/src/system/takeover/sandbox.rs` | 117 | Win32 Shell 通知区接管（`Shell_TrayWnd` / `ToolbarWindow32`） |
| `crates/openrevo-core/src/system/color_calibration.rs` | 56 | `mscms.dll` + ICM 注册表 + GDI `SetDeviceGammaRamp` |
| `crates/openrevo-core/src/driver/ble_cooler.rs` | 32 | WinRT BLE GATT（`BluetoothLEAdvertisementWatcher`） |
| `crates/openrevo-core/src/driver/uefi_nvram.rs` | 19 | `GetFirmwareEnvironmentVariableW` |
| `crates/openrevo-core/src/system/switches/radio.rs` | 12 | WinRT 无线电 / 飞行模式 |
| `crates/openrevo-core/src/system/switches/mod.rs` | 10 | 开关汇总 + `CommandExt` |
| `crates/openrevo-core/src/system/process.rs` | 9 | `CreateToolhelp32Snapshot` 进程枚举 |
| `crates/openrevo-core/src/hal/declarative/wmi_mailbox.rs` | 9 | WMI 邮箱总线 |
| `crates/openrevo-core/src/system/autostart.rs` | 8 | `HKCU\...\Run` 注册表 |
| `crates/openrevo-core/src/system/switches/{touchpad,camera,audio}.rs` | 7/4/4 | HID 开关、DirectShow、CoreAudio |
| `crates/openrevo-core/src/system/{display,service_mgr,hardware_probe}.rs` | 6/5/4 | `ChangeDisplaySettings`、SCM、WMI 机型探测 |
| `crates/openrevo-core/src/driver/{hid_ffi,gpu_bridge}.rs` | 6/6 | HID/IOCTL、NVAPI |
| 其余（wmi_ip3、wmi_event_listener、wmi_mulong、power/*、streamer/tracker、takeover/lifecycle） | 16 | 零散 Win32 |

全仓 `target_os = "linux"` 出现次数：**0**。`windows = "0.58"` 在 `src-tauri`、`openrevo-main`、`openrevo-osd`、`openrevo-widget` 四个 manifest 中是**无条件依赖**（仅 `openrevo-core` 已写成 `[target.'cfg(windows)'.dependencies.windows]`）。

### 0.2 可移植性判断

**省事的三处结构（作者已经留好缝）**

1. `hal/traits.rs` 的 `HardwareDriver` / `MechrevoHardwareHAL` 是纯平台无关联 trait，物理地址全部收敛在钥匙库（`hal/vault.rs`），上层不碰寄存器。照 §5 的说法，这些地址在 Linux 下直接有效。
2. Slint 界面是**独立子进程**（`openrevo-main` / `openrevo-osd` / `openrevo-widget` 三个 bin，经 stdin/stdout JSON 通信，见 `src/system/slint_window/mod.rs:157-194`），不跟 Tauri 抢主线程事件循环。winit 后端在 Wayland 下可用，双循环冲突问题天然不存在。
3. 前端是 React + Vite，命令全走 Tauri `invoke`，与平台无关；`openrevo-ffi` 本来就要在 Linux 产出 `libopenrevo_core.so`。

**必须重写的四处耦合**

1. `windows` crate 无条件依赖 → 所有含 `windows::` 的模块在 Linux 首行就炸。
2. `takeover/sandbox.rs` 整套是 Windows 通知区域窗口父子关系（`Shell_TrayWnd` → `ToolbarWindow32`），KDE 的托盘是 SNI，语义不同。
3. 色彩、显示、电源方案三块直连 `mscms` / `user32!ChangeDisplaySettings` / `PowerSetActiveScheme`，Linux 分别对应 colord、DRM/KWin 脚本、power-profiles-daemon。
4. 机型探测、EC 读写全走 WMI（`wmi_ec.rs` 用 `WmiMof` 查询 `AcpiTest_MULong` 一类），Linux 需落到 `ec_sys` 直接读写。

### 0.3 阶段划分

`P0 环境` → `P1 编译零错误` → `P2 Linux HAL` → `P3 子系统替换` → `P4 Wayland 专项` → `P5 打包与权限` → `P6 验证`。

门槛定义：P1 只求 `cargo check --workspace` 通过（功能可为 no-op），P2 起才谈真实硬件，避免在编译期同时处理设计问题。

---

## P0 环境基线（已完成）

Arch Linux x86_64，rustc 1.100.0 / cargo 1.100.0，全部经 `pacman` 管理：

```bash
sudo pacman -S --needed rust webkit2gtk-4.1 librsvg libappindicator-gtk3 gtk-layer-shell
```

- `webkit2gtk-4.1` / `javascriptcoregtk-4.1` 是 Tauri v2 在 Linux 的硬前置（已确认 2.54.3）。
- crates 镜像写 `~/.cargo/config.toml`（rsproxy sparse），仓库下载走 `ghfast.top`，push 走 SSH。
- 编译验证不需要图形会话；跑起来需要真实 KDE Wayland 会话，本机为 TTY，运行期验证在目标机进行。

---

## P1 编译基线清零（已完成）

> 目标：`cargo check --workspace` 通过。允许 no-op，但**公共 API 签名必须与 Windows 版逐字一致**，上层调用点不改。

### 1.1 先立平台层 `pal`

新增 `crates/openrevo-core/src/pal/{mod.rs,windows.rs,linux.rs}`：

```rust
pub trait EcPort: Send + Sync {
    fn read_byte(&self, addr: u8) -> Result<u8, HalError>;
    fn write_byte(&self, addr: u8, val: u8) -> Result<(), HalError>;
    fn is_available(&self) -> bool;
}
pub trait ProcessPort: Send + Sync {
    fn snapshot(&self) -> Vec<(u32, String)>;   // pid, exe stem（小写）
}
pub trait AutostartPort: Send + Sync {
    fn set(&self, name: &str, cmd: &str, enabled: bool) -> Result<(), HalError>;
    fn is_enabled(&self, name: &str) -> bool;
}
pub fn ec() -> &'static dyn EcPort;            // cfg 分派，Linux 返回 EcSysPort
pub fn processes() -> &'static dyn ProcessPort;
```

`pal/mod.rs` 用 `#[cfg(windows)] use windows::...` / `#[cfg(target_os = "linux")] use linux::...` 做单点选择，业务模块只依赖 trait。

### 1.2 逐文件处置表

| 文件 | 处置 | Linux 行为 |
| :--- | :--- | :--- |
| `system/takeover/sandbox.rs` | 整模块 `#[cfg(windows)]`，新增 `takeover/sandbox_linux.rs` | 空实现：`sandbox_status()` 返回 `NotApplicable`（KDE 无通知区窗口可接管） |
| `system/takeover/{lifecycle,sniffer}.rs` | 同上门控 | 仅保留 `probe()` → `None` |
| `system/color_calibration.rs` | 拆 `color_calibration/{mod,windows,linux}.rs` | Linux：colord D-Bus（`org.freedesktop.colord`）列设备、挂 ICC；无 colord 则返回未校准 |
| `driver/ble_cooler.rs` | 拆 `ble_cooler/{mod,windows,linux}.rs` | Linux：BlueZ D-Bus `org.bluez` GATT（`StartNotify` / `WriteValue`） |
| `driver/uefi_nvram.rs` | cfg 门控 + `efivarfs` 分支 | 读写 `/sys/firmware/efi/efivars/<name>-<guid>` |
| `system/switches/radio.rs` | cfg 门控 + rfkill 分支 | `/dev/rfkill` ioctl 或 `rfkill` 状态读 `/sys/class/rfkill/*/soft` |
| `system/switches/{camera,touchpad,audio}.rs` | cfg 门控 + 对应分支 | 摄像头：`/sys/bus/usb` 端口电源或 v4l2；触控板：libinput/X11 `xinput`；麦克风：`pactl set-source-mute` |
| `system/process.rs` | 走 `pal::processes()` | 读 `/proc/*/comm` |
| `hal/declarative/wmi_mailbox.rs` | Linux 下切 `pal::ec()` 字节通道 | 邮箱协议字节序列不变，底换成 EC 端口 |
| `driver/wmi_*.rs` | 整文件 `#[cfg(windows)]` | 由 P2 的 `hal/linux_ec.rs` 取代 |
| `system/autostart.rs` | 走 `pal::autostart()` | 写 `~/.config/autostart/<name>.desktop` |
| `system/service_mgr.rs` | cfg 门控 | systemd：`systemctl` 探测 `uwacpi.service` 等价物，驱动安装段 no-op |
| `system/display.rs` | cfg 门控 + `kscreen-doctor`/`wlr-randr` 分支 | 枚举 `kscreen-doctor -j`，应用模式用 `kscreen-doctor output.X.mode.Y` |
| `driver/{hid_ffi,gpu_bridge}.rs` | Win32 部分门控 | HID 走 `/dev/hidraw*`；NVAPI 段由 NVML（`libnvidia-ml.so`，已有 `nvml.rs`）承担 |
| `driver/nvapi.rs` | 编译期排除 | NVML 已独立成模块，不移植 NVAPI |
| `system/hardware_probe.rs` | 读 DMI | `/sys/class/dmi/id/{product_name,board_name,bios_version}`、`/proc/cpuinfo` |
| `system/{power,power_mode}/*` | 见 P3 | Windows 电源方案 → PPD |
| `src-tauri/src/**`（应用层 229 文件中的 40 余处） | P3/P4 | 托盘、快捷键、窗口、OSD |

### 1.3 manifest 收敛

四个 manifest 的 `windows` 依赖改 `[target.'cfg(windows)'.dependencies.windows]`；`openrevo-main` / `-osd` / `-widget` 另加 `[target.'cfg(target_os = "linux")'.dependencies]` 里的 X11/Wayland 备用 crate（winit 自带，暂不额外引）。

### 1.4 验收

`cargo check --workspace` 零错误零警告（`unused import` 一并清）。此步不提交推送，与 P2 一起过 review。

### 1.5 实际落地（与 1.1 的偏差，已执行）

§1.1 设计的是 trait object 端口表，落地时改成了**同名同签名的自由函数**：trait 需要厚一层 `dyn` 装箱，而所有调用点都只要一个同步函数，自由函数让 Windows 分支能原样搬进 `#[cfg(windows)]`（`pal/ec.rs` 的 `windows_arm` 就是这么保住 IOCTL 与 MULong 回退的），跨平台调用点一行不改。落地的 `pal/` 全部是这一形态：

- `pal/ec.rs`（EC 端口）、`pal/process.rs`（`snapshot`/`terminate`/`oem_installed`/`run_quiet`/`command`）、`pal/reg.rs`（`RegRoot` + 读写/枚举/备份）、`pal/firmware.rs`（efivarfs / pnputil↔pci rescan / GPU 服务 / reboot）、`pal/display_color.rs`（gamma、ICC 关联）、`pal/xdg.rs`（freedesktop basedir）、`pal/power.rs`（执行状态抑制、空闲时刻）、`pal/input.rs`（按键查询）。

模块处置也从"整模块搬走"改成三档，按耦合度选：**整文件 `#[path]` 拆分**（`display`、`wmi_*`、`hooks`、`power_listener`、`win32_tray`、`webview_dock`、`openrevo-main/window`）、**同文件 `#[cfg]` 门控 + 底部同名孪生**（`sandbox`、`service_mgr`、`color_calibration`、`ble_cooler`、`hid_ffi`、`switches/*`、`cloud_icc`、`display_tuning`）、**纯桩**（`bld_driver_mgr` 的 SCM、`hid_ffi` 的 Win32 FFI）。

链接是独立关卡：`cargo check` 绿了不代表能出 `.so`。`openrevo-core` 的 `crate-type` 只有 `lib`，rlib 不链接，所以 `#[link(name="hid"/"setupapi"/"kernel32"/"user32"/"advapi32")]` 的缺失一直是隐形的；必须 `cargo rustc --crate-type cdylib` 才会暴露 `ld.lld: error: unable to find library -lhid` 之类。P1 的验收因此加一条：**cdylib 链接通过**。

---

## P2 Linux HAL 驱动

> 依据 §5 与 `REFERENCES.md` §1：**先走内核对同方的 WMI 直通，再退回 `ec_sys` 裸 EC**。
> 裸 EC 是最后手段——每次写都是一次 ACPI 事务，且 ec_sys 无内核锁。

驱动优先级（`probe()` 逐个尝试，第一个成功的即锁定整个会话）：

1. **`TongfangWmiDriver`** — 存在 hwmon chip `tongfang_mifs` 即命中。
   - 读写全走 sysfs，不含任何 ACPI 代码：`temp1_input` / `fan{1,2,3}_input`、`/sys/firmware/acpi/platform_profile`、WMI 设备目录下的 `gpu_mode`、`kb_mode`、`fan_boost`、`/sys/class/leds/laptop::kbd_backlight/brightness`。
   - 应用的四档电源模式映射到 `platform_profile`：办公→`LOW_POWER`、均衡→`BALANCED`、狂暴→`BALANCED_PERFORMANCE`、极客→`PERFORMANCE`。
   - DC 供电时切高性能/全速会被固件拒绝（`EOPNOTSUPP`），前端要按 `SYSTEM_AC_TYPE` 置灰而非报错。
   - 驱动未进 mainline（`REFERENCES.md` §1），所以本路径**可能整条缺席**，必须有后备。
2. **`LinuxEcDriver`** — `sudo modprobe ec_sys write_support=1`，节点 `/sys/kernel/debug/ec/ec_sys`。
   - 偏移与掩码全部取自 `vault.rs` 已登记的物理地址；读写走 `pal::ec()`。
   - `EcSysPort` 内置 `Mutex<File>` + 200 µs 写后延时（沿用 `ec_bridge.rs` 现有节流参数，不改时序）。
3. **`MockHAL`** — 无硬件（本开发机就是这种情况：VirtualBox，无 WMI、无 EC）。

落地要求：
- 机型探测走 `/sys/class/dmi/id/{sys_vendor,product_name,board_name}`，只放行同方/机械革命/Uniwill 系，避免误写他人 EC。
- `hal/mod.rs` 选择链按上面顺序插入 `#[cfg(target_os = "linux")]` 分支，日志必须写明最终选中了哪条路径。
- 热键：驱动提供的 input 设备 `Tongfang MIFS WMI hotkeys` 只上报 `KEY_PROG1/KEY_CALC/KEY_WWW`，其余 Fn 组合键是 `KE_IGNORE`，**只能旁路监听 evdev 原始事件**，不能指望它给全量键。
- 权限：`ec_sys` 走 udev 规则或组（P5），否则 debugfs 需 root。

验收：编译通过 + `MockHAL` 降级路径可跑；真机上 `hardware_probe` 命中 `TongfangWmiDriver` 并读到双风扇转速与四档模式。

---

## P3 子系统替换（进行中）

| 子系统 | Windows 现状 | Linux 方案 |
| :--- | :--- | :--- |
| 电源模式 | `PowerSetActiveScheme` + EC 寄存器 | `power-profiles-daemon` D-Bus（`net.hadess.PowerProfiles.ActiveProfile`）+ EC 双写 |
| 电池阈值 / 风扇强冷 | WMI `AcpiTest_MULong` | EC 同一地址（钥匙库） |
| 托盘 | `Shell_NotifyIcon` + 自绘菜单 | SNI（`ksni` crate，纯 Rust）。**不能用 Tauri 的 `tray-icon`**：它经 libappindicator 注册 `ItemIsMenu`，点击事件与图标坐标双双丢失（见 `REFERENCES.md` §2.1） |
| 全局快捷键 | `RegisterHotKey` | X11：`XGrabKey`；Wayland：`org.freedesktop.portal.GlobalShortcuts`（persist token 存配置） |
| 显示 | `ChangeDisplaySettingsEx` | `kscreen-doctor` / `wlr-randr`，KDE 优先 |
| 色彩 | `mscms` + GDI gamma ramp | colord + KWin `ApplyGamma`（KWin 脚本经 D-Bus） |
| 开机自启 | `HKCU\Run` | `~/.config/autostart/*.desktop` |
| 服务/驱动 | SCM + PnP `.sys` | systemd unit + udev，无驱动可装 |
| 机型探测 | WMI `Win32_ComputerSystem` | DMI 文件 |
| 窗口定位 | `SetWindowPos` 绝对坐标 | `tauri` 逻辑坐标；Wayland 下无绝对定位，用 `set_position` 尽力 + 居中兜底 |

`system/takeover/*` 整体降级为 `NotApplicable`：KDE 没有可被接管的 OEM 通知区域。

### P3 交付状态

已落地（`1c86db6`、`01bcb2d`）：

- 托盘：`src/system/tray_linux.rs`，`ksni` 0.2 的 StatusNotifierItem，菜单项与 Windows 版一一对应。`perform()` / `current_menu_state()` / `TrayAction` 抽到平台无关的 `win32_tray.rs` 门面，两个后端共用。
- 睡眠/唤醒：`src/system/power_events/`，logind `PrepareForSleep` / `PrepareForShutdown` 信号（`dbus` 0.9，系统总线），合盖读 `/proc/acpi/button/lid/*/state`。`sleep_guard`/`wake_guard`/`resume_pipeline` 等状态机逐字抽出共用。
- 电源抑制：`pal::power::set_execution_state` 用 `systemd-inhibit --mode block sleep infinity` 子进程，状态映射 `Normal`/`KeepAwake`/`ScreenOffAway`。
- 自启：`system/autostart_linux.rs` 写 `$XDG_CONFIG_HOME/autostart/openrevo.desktop`。
- 服务接管：`systemctl --user mask --now`，unit 名按 `OEM_TASKS` 探测。
- 显示：`system/display.rs` 的 DRM/EDID 分支（`wlr-randr` → `kscreen-doctor` → `xrandr`）、亮度走 `/sys/class/backlight`。
- 色彩：gamma 写 `/sys/class/drm/<conn>/gamma`；ICC 关联落 `<XDG data>/openrevo/associations.conf`。
- 设备开关：触摸板走 EC 位，无线电走 `/sys/class/rfkill`，摄像头走 `/sys/class/video4linux`，麦克风走 `wpctl`/`pactl`。
- 文件对话框：`zenity` → `kdialog`。

未落地（仍是空缺，不是完成）：

- **热键**：`hooks_linux` 只转调 `init_shortcuts`，evdev 抓取与 portal `GlobalShortcuts` 都还没接。
- **空闲检测**：`pal::power::last_input_tick` 返回 `None`，所以屏灭后"用户动了"的判定退化为未知。
- **BlueZ**：`ble_cooler` 的扫描/连接一律返回 Err，蓝牙水冷在 Linux 当前不可用。
- **colord**：ICC 安装只写关联文件，没让 colord 真挂载。
- **native_wry**：WebView2 合成窗口在 Linux 仍是待办，插件宿主改由 Tauri 独立 toplevel 承担（`webview_dock` 的 Linux 分支）。

---

## P4 Wayland / KDE 专项

1. **OSD / 小组件**：Slint 后端是 winit，`slint` 开 `unstable-winit-030`，`WinitWindowAccessor::winit_window()` 取 `&winit::window::Window` → `raw_window_handle` → `wayland-protocols` 的 `zwlr_layer_shell_v1`，`Layer::Overlay`、`KeyboardInteractivity::None`、`exclusive_zone=-1`。
   - 若嫌自己接协议麻烦：Tauri 侧窗口可用 `libloading` 动态加载 `libgtk-layer-shell.so.0`，调用序列与常量照抄 `docs/linux/ref/gglib-layer-shell.rs`（`LAYER_OVERLAY=3`、`KEYBOARD_ON_DEMAND=2`、必须 `gtk_layer_is_supported()` 先探，Mutter/X11 直接退回）。
2. **主界面保持 Slint，且必须可自由缩放**（详见 P4.5）。
3. **托盘单击行为**：SNI `Activate` 提供点击坐标，左键唤起主窗；`dismiss_windows_tray_overflow` 在 Linux 恒 no-op。
4. **快捷键**：portal 会话恢复（`~/.config/openrevo/shortcuts.toml` 存 token），无 portal 时降级 X11 grab 并提示。
5. **WebView**：Linux 是 WebKitGTK。检查 `native_wry`（Windows 专属 WebView2 合成）在 Linux 的替代：改用 `tauri::WebviewWindowBuilder` + `unstable` 特性；`window_open_revo` 注入的 Import Map 逻辑保留。
6. **分数缩放**：前端 `devicePixelRatio` 处理，KDE 125%/150% 下 OSD 与主窗不重叠。
7. **沙盒剪贴板/拖拽**：`data-tauri-drag-region` 在 Wayland 需 compositor 支持，拖拽失败时不阻塞主流程。

### P4.5 主界面：Slint + 自由缩放

现状（`crates/openrevo-main/ui/main_window.slint:14-28`）：

```slint
export component MainWindow inherits Window {
    no-frame: true;
    background: transparent;
    width: is_expanded ? 1120px : 395px;
    height: is_expanded ? 760px : 632px;
```

根尺寸被 `is_expanded` 硬绑死，窗口不可缩放；`no-frame: true` 在 Wayland 上等于连边框都没有，用户无处下手指拖拽。

目标：**界面结构照原作者的 Slint，尺寸改成"初始值 + 可缩放 + 有下限"**。

1. 尺寸语义：
   - 保留 `is_expanded` 作为"默认形态"：首次显示仍按 395×632 / 1120×760 起。
   - 但把硬绑定改成 `in-out property <length> win_width / win_height`，初始由 `is_expanded` 决定，之后**由用户拖拽驱动**；`is_expanded` 切换只在用户没手动改过尺寸时生效（加 `user_resized: bool`）。
   - 加 `min-width` / `min-height`（抽屉 320×480，仪表盘 900×600），避免 UI 崩坏。
2. 布局：`DrawerView` / `FullDashboard` 已经是 `width:100%; height:100%` 跟着根走，内部绝大多数是百分比与 layout，只需排查 `grep "width: [0-9]"` 命中的少数固定宽度（`sidebar.slint` 8 处）改成 `min-width`/弹性。
3. 缩放入口（Wayland 没有服务端边框就没有拖拽区）：
   - 首选：`slint` 的 `unstable-winit-030` + `winit::window::Window::drag_resize_window(ResizeDirection)`，在 Slint 里自绘 8 个透明热区（左右上下 + 四角），按下时调 resize。这是 Wayland 上唯一能自定义 CSD 缩放手势的路径。
   - 兜底：Linux 上把 `no-frame` 在 X11 下关掉（让 WM 画边框），Wayland 下若脚本检测到 `XDG_SESSION_TYPE=wayland` 且 CSD 不可用，就给一个 KDE 窗口规则提示（用户可右键标题栏→更多操作→调整大小）。
4. 圆角/透明：`background: transparent` + Windows 的 DWM 圆角在 Linux 无对应物；Wayland 上由合成器按 `rounded-corner`（KWin 有 `BorderRadius` 规则）或自绘。不要在 Linux 路径调用任何 `Dwm*`。
5. Windows 专属代码隔离：`crates/openrevo-main/src/lib.rs:3` 直接 `use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};`，`src/window.rs` 全文是 HWND 子类化。二者都要 `#[cfg(windows)]`，Linux 提供空实现（`apply_window_decorations`、`find_thread_hwnd`、`align_mini_drawer_to_tray`、`expand_native_window` 等同名 no-op），保证调用点零改动。

---

## P5 打包与权限

1. `PKGBUILD`（`openrevo` / `openrevo-git`）：`depends=(webkit2gtk-4.1 gtk3 power-profiles-daemon bluez colord)`，`makedepends=(rust nodejs pnpm)`，装 `/usr/bin/open-revo`、`/usr/share/applications/*.desktop`、图标、三个伴生 bin。
2. `udev` 规则：`pwris`/`hidraw`/`backlight` 的 `uaccess` 标签；`/etc/modules-load.d/openrevo.conf` 写 `ec_sys`。
3. `modprobe` 参数 `write_support=1` 由 `modprobe.d/openrevo.conf` 提供。
4. 特权写走 polkit action `com.openrevo.ecwrite`，helper 单 bin，用户态失败才提权。
5. `systemd --user` unit 承载常驻守护；`Restart=on-failure`。
6. CI：`.github/workflows/release.yml` 加 `build-linux` job（`ubuntu-24.04`，装 `libwebkit2gtk-4.1-dev`，`pnpm build` + `cargo build --release --workspace`，产物打 tar.zst + deb）。

---

## P6 验证矩阵

| 级别 | 项 | 通过标准 |
| :--- | :--- | :--- |
| 编译 | `cargo check --workspace` / `cargo clippy` | 零错误 |
| 单测 | `pnpm test`（7 个 `tests/*.test.mjs`） | 全绿，不改断言 |
| 前端 | `pnpm build` | tsc + vite 通过 |
| 运行 | KDE Wayland 启动 | 主窗、托盘、OSD、快捷键可用 |
| 硬件 | EC 读、风扇转速、模式切换 | 与 Windows 侧读数一致 |
| 回归 | 声明式 manifest 机型 | Linux 下正确走 manifest 分支而非 EC 驱动 |

---

## 风险

1. **`ec_sys` 需 root/debugfs**：未配置 udev 时功能全空，必须在 P5 之前完成权限，否则 P2 验收不可复现。
2. **`ec_sys` 无锁**：并发 SC/EC 写会丢字节，靠 `EcSysPort` 串行化与写延时节流；`wmi_ec` 的重试语义要在 Linux 分支保留。
3. **portal 全局快捷键不通用**：Plasma 支持，其他 compositor 不保证；降级路径必须有。
4. **Slint + winit 在 Wayland 的窗口定位**：`set_position` 可能被 compositor 忽略，OSD 位置要按屏幕比例兜底。
5. **NVAPI 不可用**：MUX 切换与部分功耗读写无 Linux 等价物，先返回 `UnsupportedModel`，不做假实现。
6. **`webkit2gtk` 版本跳变**：Arch 滚动更新可能超前 Tauri 期望，锁定 `webkit2gtk-4.1` ABI 并在 PKGBUILD 记 `depends`。

---

## 交付物

- `docs/linux/PORT-PLAN.md`（本文件）
- `docs/linux/BUILD.md`：从零构建、跑起来、打包的操作步骤
- 平台层与各子系统改动，按阶段分提交；`main` 保持编译分支不动，全部落在 `wayland-kde`
