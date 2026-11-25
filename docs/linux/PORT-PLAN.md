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
2. **`LinuxEcDriver`** — 先认 `/sys/kernel/debug/ec/ec_sys`（`modprobe ec_sys write_support=1`），没有再退 `ec0/io`。
   - 偏移与掩码全部取自 `vault.rs` 已登记的物理地址；读写走 `pal::ec()`。
   - `EcSysPort` 内置 `Mutex<File>` + 200 µs 写后延时（沿用 `ec_bridge.rs` 现有节流参数，不改时序）。
   - 见下方「EC 实测」：实机上 `ec_sys` 平铺节点**根本不出现**，只有 `ec0/io`，且 `ec0/io` 只覆盖 256 字节的一页。
3. **`MockHAL`** — 无硬件（本开发机就是这种情况：VirtualBox，无 WMI、无 EC）。

落地要求：
- 机型探测走 `/sys/class/dmi/id/{sys_vendor,product_name,board_name}`，只放行同方/机械革命/Uniwill 系，避免误写他人 EC。
- `hal/mod.rs` 选择链按上面顺序插入 `#[cfg(target_os = "linux")]` 分支，日志必须写明最终选中了哪条路径。
- 热键：驱动提供的 input 设备 `Tongfang MIFS WMI hotkeys` 只上报 `KEY_PROG1/KEY_CALC/KEY_WWW`，其余 Fn 组合键是 `KE_IGNORE`，**只能旁路监听 evdev 原始事件**，不能指望它给全量键。
- 权限：`ec_sys` 走 udev 规则或组（P5），否则 debugfs 需 root。

验收：编译通过 + `MockHAL` 降级路径可跑；真机上 `hardware_probe` 命中 `TongfangWmiDriver` 并读到双风扇转速与四档模式。

### EC 实测（MECHREVO 同方实机，2026-11）

这台机器上三条预设路径全部落空，最后走的是**固件自己给 WMI 留的那条**：

| 路径 | 实测结果 |
| :--- | :--- |
| `tongfang_mifs` hwmon | 不存在。内核自带的 `uniwill-laptop.ko` 只 alias `dmi:svnTUXEDO*`，本机 `svn=MECHREVO` → `No such device` |
| `ec_sys` 平铺节点 | `modprobe ec_sys write_support=1` 后仍无 `ec_sys`；`/sys/kernel/debug/ec/` 下只有 `ec0/{gpe,io,use_global_lock}`，`io` 是 0700 root |
| EC 端口 0x62/0x66 | 判死。写命令后状态寄存器变 `0x0a`（BURST+IBF）随后恒 `0x00`，**OBF 永不置位**——内核 `acpi_ec` 持有 EC 并抢走了数据 |
| **`AcpiTest_MULong`（WMI）** | **可用**。`/proc/acpi/call` 调 `\_SB.AMW0.WMBC 0x0a 0x04 {40 字节}` |

固件侧反编译（`acpica` 的 `dsdt.dsl`，23 个 SSDT 均已核）给出了完整落点：`AMW0._WDG` 里 `ABBC0F6F-8EA1-11D1-00A0-C90629100000` 的 ObjectId 是 `BC`、instance 10，即 Windows 侧的 `AcpiTest_MULong`。调用链 `WMBC(Arg1=4) → OEMG → RKBC`（读）/ `WKBC`（写），寄存器是 bank-0 邮箱：`LDAT=0x8a`、`HDAT=0x8b`、`FLAGS=0x8c`（bit0 RFLG、bit1 WFLG、bit2 BFLG、bit3 CFLG、bit7 DRDY）、`CMDL=0x8d`、`CMDH=0x8e`，全程持固件互斥 `\ECMX`，轮询间隔就是固件里的 `Sleep(0x32)`。这与 TUXEDO 的 `uniwill_wmi.c` 同构，所以不必自己发明协议。

由此得到两条硬结论：

1. **一次读返回的是 16 位对**：地址 `A` 的读给出 `CMDL=byte[A]`、`CMDH=byte[A+1]`。所以 `ADDR_*_B1`/`_B2` 的语义是"高字节/低字节"，转速 `= (byte[A] << 8) | byte[A+1]`（大端取），不是两个独立寄存器。用 `stress-ng --cpu 20` 把封装温度从 45 °C 拉到 82 °C 验证过：`0x464` 与 `0x46c` 各自从约 1900 RPM 爬到 4100~4600 再回落，两路互不干扰。
2. **邮箱页 0x04 就是 `ec0/io` 的那 256 字节**：`ec0/io` 前 16 字节与 AML 读 `0x400…0x40f` 完全相同。因此只要地址落在 `0x400–0x4ff`，读 `ec0/io` 的对应偏移即可（0.6 ms），不必每次付固件那 50 ms 的 `Sleep`；其它页只能走 AML。

`/proc/acpi/call` 有个反直觉的读语义（`acpi_call.c:339` 起）：缓冲区小于结果长度时它 `return 0` 而不是短读，读满一次还会把结果清空。所以 Rust 侧**必须一次性 `read` 进 ≥512 字节缓冲**再解析——`read_to_string` 内部先读 32 字节，永远拿到空串（表现为"shell `cat` 正常、程序拿不到"）。

### 权限：常驻 broker，不是 pkexec

读路径每秒要取十几个寄存器，每次 `pkexec` 一个进程加一次授权完全不可行；而两条通道都是 root-only。所以 `openrevo-ec-helper` 增加了 `serve` 模式：root 常驻，监听 `/run/openrevo/ec.sock`（`root:openrevo-ec`、`0660`），**能否连接就是授权模型**——组员连得上，非组员连不上。客户端在 `pal::ec()` 里保持长连接，行协议 `r <addr16>` / `w <addr16> <byte8>…`（单批不超 64 对）/ `probe`，畸形请求回 `err` 但不断连接。

broker 进程必须禁掉自己的客户端（`pal::ec::linux::disable_broker_client()`），否则 `read()` 会经 `broker_read` 连回自身 socket，逐层递归到 fd 耗尽——这个坑踩过一次，初读从 257 ms 涨到 10.7 s，backtrace 里全是 `broker_request`。

配套分发：`packaging/linux/openrevo-ec.service`（`RuntimeDirectory=openrevo`、`Group=openrevo-ec`、`CapabilityBoundingSet=CAP_SYS_RAWIO`、`ProtectSystem=strict` + `ReadWritePaths=/sys /proc`）、`packaging/linux/sysusers-openrevo.conf`（建 `openrevo-ec` 组）、pacman 的 `.install` 脚本与 deb/rpm 的 post 脚本负责把桌面用户加进组（幂等）。**装完要重新登录**，组身份才会生效。

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
- EC 读写：`pal::ec()` 三级通道（debugfs 节点 → AML 邮箱 → 端口）+ 常驻 broker（见 P2「EC 实测」）。实机已验证：读 `0x740` 57 ms、读节点页 `0x464` 0.6 ms、写 `0x7a6` 幂等回读一致。
- GPU 遥测：`driver/nvml.rs` 在 Linux 加载 `libnvidia-ml.so.1`（原来只试 `nvml.dll`，必失败），不再需要 `nvidia-smi` 子进程；`VBIOS TGP` 50/115 W 可直接读到。
- 温度：`ec_bridge` 的 `cpu_temp`/`gpu_temp` 原本是写死的 45/42 常量，现在每轮轮询取 `coretemp` 的 `Package id 0`（跳过 `acpitz`，同方主板报固件常量）与 NVML。
- 配置目录：`resolve_config_dir()` 在 Linux 上曾落到 `PathBuf::from(".")`，把 `config.json`/`models.json`/`plugins` 全写进启动目录；现在走 `pal::xdg::config_dir()` → `~/.config/openrevo`。

热键与输入（`ae9d9fc` 之后）：`pal/evdev.rs` 被动读 `/dev/input/event*`（不 `EVIOCGRAB`），`pal/keymap.rs` 把 evdev code 折回项目已有的 Win32 VK 词表，`hooks_linux.rs` 复刻锁键去抖与 toast。空闲时刻来自同一事件流，`pal::power::last_input_tick` 不再恒 `None`。**portal `GlobalShortcuts` 仍未接**：它只在有会话 portal 的桌面可用，且需要持久 token，属于后续增强，不影响 evdev 路径。

未落地（仍是空缺，不是完成）：

- **BlueZ**：`ble_cooler` 的扫描/连接一律返回 Err，蓝牙水冷在 Linux 当前不可用（用户明确说过不急）。
- **colord**：ICC 安装只写关联文件，没让 colord 真挂载；gamma 已能直写 DRM，跨桌面着色仍靠直写。
- **native_wry**：插件宿主在 Linux 由 Tauri 独立 toplevel 承担；`native_wry` 的 wry 合成路径在 Linux 已允许跨线程建 GTK event loop（`EventLoopBuilderExtUnix::with_any_thread`），但未在真机验证。
- **layer-shell OSD**：winit 0.30 不暴露 `wl_surface`，所以 OSD 仍是普通 toplevel + `always-on-top`，定位交给合成器策略。

---

## P4 Wayland / KDE 专项

1. **OSD / 小组件**：**未落地**。原计划的 `unstable-winit-030` + `zwlr_layer_shell_v1` 路线被证伪了一半：`WinitWindowAccessor::winit_window()` 确实能拿到 winit 窗口，但 **winit 0.30 不公开 `wl_surface`**（`platform/wayland.rs` 只有 `is_wayland()`、`with_wayland()` 之类），所以拿不到可包成 `zwlr_layer_surface_v1` 的代理。可行路线只剩两条，都需要额外进程或额外重写：
   - 用 `gtk-layer-shell` 写一个独立 GTK OSD 进程，替换现有的 Slint OSD（照抄 `docs/linux/ref/gglib-layer-shell.rs`：`LAYER_OVERLAY=3`、`KEYBOARD_ON_DEMAND=2`、先 `gtk_layer_is_supported()`）。
   - 等 winit 暴露 `wayland_surface()` 后自接协议。
   当前交付：OSD 是普通 toplevel + `always-on-top: true`（`crates/openrevo-osd/ui/osd.slint:5-7`），固定 640×130，定位由合成器决定（KWin 下新 toplevel 居中）。
2. **主界面保持 Slint，且必须可自由缩放**（详见 P4.5）。
3. **托盘单击行为**：SNI `Activate` 提供点击坐标，左键唤起主窗；`dismiss_windows_tray_overflow` 在 Linux 恒 no-op。
4. **快捷键**：portal 会话恢复（`~/.config/openrevo/shortcuts.toml` 存 token），无 portal 时降级 X11 grab 并提示。
5. **WebView**：Linux 是 WebKitGTK。检查 `native_wry`（Windows 专属 WebView2 合成）在 Linux 的替代：改用 `tauri::WebviewWindowBuilder` + `unstable` 特性；`window_open_revo` 注入的 Import Map 逻辑保留。
6. **分数缩放**：前端 `devicePixelRatio` 处理，KDE 125%/150% 下 OSD 与主窗不重叠。
7. **沙盒剪贴板/拖拽**：`data-tauri-drag-region` 在 Wayland 需 compositor 支持，拖拽失败时不阻塞主流程。

### P4.5 主界面：Slint + 自由缩放

现状（`crates/openrevo-main/ui/main_window.slint:14-28`，改动前）：

```slint
export component MainWindow inherits Window {
    no-frame: true;
    background: transparent;
    width: is_expanded ? 1120px : 395px;
    height: is_expanded ? 760px : 632px;
```

根尺寸被 `is_expanded` 硬绑死，窗口不可缩放；`no-frame: true` 在 Wayland 上等于连边框都没有，用户无处下手指拖拽。

目标：**界面结构照原作者的 Slint，尺寸改成"初始值 + 可缩放 + 有下限"**。

1. 尺寸语义 —— **已落地**：`width`/`height` 的 `is_expanded ? 1120px : 395px` 绑定彻底删除，改为 `resize-border-width: 6px` + `min-width: 320px` + `min-height: 480px`；初始尺寸在 `crates/openrevo-main/src/lib.rs` 里 `window.window().set_size(LogicalSize)` 一次给足，之后的形态切换仍走 `window_linux.rs::expand_native_window` / `align_mini_drawer_to_tray`（同样只调 `set_size`），因为不再有绑定，用户拖出来的尺寸不会被弹回。原计划里的 `user_resized` 标志与 `win_width`/`win_height` 属性因此省掉。
2. 布局：`DrawerView` / `FullDashboard` 已经是 `width:100%; height:100%` 跟着根走，内部绝大多数是百分比与 layout，只需排查 `grep "width: [0-9]"` 命中的少数固定宽度（`sidebar.slint` 8 处）改成 `min-width`/弹性。
3. 缩放入口 —— **已落地，且不需要自绘热区**。Slint 1.18 的 `Window.resize-border-width` 是 winit 原生能力：`i-slint-backend-winit/src/winitwindowadapter.rs:1449` 在 `CursorMoved` 里用 `handle_cursor_move_for_resize(.., resize_border_width())` 算出方向，左键按下时 `handle_resize()` 调 winit 的 `drag_resize_window`。所以只要 `resize-border-width: 6px` + 不绑定 width/height 即可，`unstable-winit-030` 与手写协议都不用开。
   - 实测约束：绑定 `width`/`height` 会让 winit 认为窗口固定尺寸，热区失效 —— 这是必须移除绑定的真正原因。
   - 兜底：若某些合成器忽略 `xdg_toplevel.resize`，用户仍可用 Super+右键拖拽（KWin 内置），无需代码。
4. 圆角/透明：`background: transparent` + Windows 的 DWM 圆角在 Linux 无对应物；Wayland 上由合成器按 `rounded-corner`（KWin 有 `BorderRadius` 规则）或自绘。不要在 Linux 路径调用任何 `Dwm*`。
5. Windows 专属代码隔离：`crates/openrevo-main/src/lib.rs:3` 直接 `use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};`，`src/window.rs` 全文是 HWND 子类化。二者都要 `#[cfg(windows)]`，Linux 提供空实现（`apply_window_decorations`、`find_thread_hwnd`、`align_mini_drawer_to_tray`、`expand_native_window` 等同名 no-op），保证调用点零改动。

---

## P5 打包与权限

1. `PKGBUILD` —— **已落地**（`packaging/linux/PKGBUILD`，`openrevo-git`）：`depends` 只放真实硬依赖（webkit2gtk-4.1、gtk3、librsvg、libappindicator-gtk3、hicolor-icon-theme、openssl），`zenity`/`wlr-randr`/`powerprofilesctl` 之类是可选运行时不上 `depends`。装 bin、desktop、三级 hicolor 图标、udev、modprobe、modules-load、user unit、LICENSE。`makepkg --printsrcinfo` 通过。
2. `udev` 规则 —— **已落地**（`packaging/linux/60-openrevo.rules`）：hidraw（048d:600b、048d:ce00）与 `backlight` 打 `uaccess`；`/etc/modules-load.d/openrevo.conf` 写 `ec_sys`。
3. `modprobe` 参数 `write_support=1` —— **已落地**（`packaging/linux/modprobe-openrevo.conf`）。
4. 特权读写 —— **已落地，改常驻 broker**：`crates/openrevo-core/src/bin/openrevo-ec-helper.rs` 仍是唯一以 root 运行的代码，但不再靠 `pkexec` 每次拉起：`serve` 子命令常驻监听 `/run/openrevo/ec.sock`（`root:openrevo-ec` 0660），组成员连得上即授权。请求是行协议 `r <addr16>` / `w <addr16> <value8>`（单批不超 64 对）/ `probe`，其余 token 一律拒绝且不断连接。`packaging/linux/com.openrevo.ecwrite.policy` 保留给"未装服务、临时用 pkexec"的场合；`pal::ec::helper_write` 仍只在 `OPENREVO_EC_HELPER=1` 时启用，节点、broker、邮箱、端口全失败才走到它。
5. `systemd --user` unit —— **已落地**（`packaging/linux/openrevo.service`，`Restart=on-failure`、`PartOf=graphical-session.target`、不带 `--visible` 即零占用待机）。与 XDG autostart 二选一，同时开会起两个进程（single-instance 会折叠）。
6. CI —— **已落地**：`.github/workflows/build-linux.yml`（check/test/clippy + Arch 容器打包）与 `.github/workflows/release.yml`（新增 `build-linux` job 出 `OpenRevo-<tag>-x86_64.pkg.tar.zst` + `checksums-linux.txt`；发布步骤抽成 `publish` job，`needs: [build-windows, build-linux]` 后统一挂到 Release，避免两个 job 并发写同一个 release）。`PKGBUILD` 的 `_repo`/`_commit` 可由环境覆盖，CI 因此构建被测 commit 而不是钉死的 ref。

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

1. **EC 通道全是 root-only**：`/sys/kernel/debug` 是 0700，`/proc/acpi/call` 是 0660 root:root。udev 改不了 debugfs，所以权限统一由 `openrevo-ec` 组加常驻 broker 解决；`/etc/modules-load.d` 里的 `ec_sys` 只影响 `ec0/io` 是否出现。组成员资格要重新登录才生效，否则 HAL 只会安静地回落到 `MockHAL`。
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
