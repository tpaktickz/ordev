# OpenRevo Linux 参考实现调研（2026-11）

> 原则：不自己发明。下面每条都指向可读的源码或已合并/在审的内核代码，写明"借什么"。
> 抓取的原文存档在 `docs/linux/ref/`。

## 1. 内核侧：Tongfang MIFS WMI（最重要）

**结论**：同方 MiInterface 已经有完整的内核驱动补丁，暴露的 sysfs/hwmon 就是我们要的上层 API。**优先走它，不要一上来写裸 EC。**

- 补丁：`platform/x86: tongfang-mifs-wmi`，作者 Mingyou Chen <qby140326@gmail.com>，v9 于 2026-02-05 投递。原文存档 `docs/linux/ref/tongfang-mifs-wmi-v9.patch.html`。
- **截至 2026-11 未进 mainline**（`torvalds/linux` 的 `master` 里 `drivers/platform/x86/tongfang-mifs-wmi.c` 仍 404）。所以真实机器上要么用它 + DKMS 自建，要么退回 `ec_sys`。
- GUID：
  - 控制 `B60BFB48-3E5B-49E4-A0E9-8CFFE1B3434B`
  - 事件 `46C93E13-EE9B-4262-8488-563BCA757FEF`
- ACPI 调用：`wmidev_invoke_method(wdev, 0, 1, &in, &out)`；输入输出都是 32 字节 packed：
  ```c
  struct tongfang_mifs_input  { u8 reserved1; u8 operation; u8 reserved2; u8 function; u8 payload[28]; };
  struct tongfang_mifs_output { u8 reserved1; u8 operation; u8 reserved2; u8 function; u8 data[28]; };
  ```
  `operation`：`GET = 250`、`SET = 251`。
- function id（**这是 OpenRevo `hal/vault.rs` 的 EC 地址在 Linux 上的等价物**）：
  | id | 名称 | 用途 |
  |---|---|---|
  | 8 | `SYSTEM_PER_MODE` | 性能模式：0 Balanced / 1 Performance / 2 Quiet / 3 FullSpeed |
  | 9 | `GPU_MODE` | 0 hybrid / 1 discrete / 2 uma（改完需重启） |
  | 10 | `KBD_TYPE` | |
  | 11 | `FN_LOCK` | |
  | 12 | `TP_LOCK` | 触摸板 |
  | 13 | `FAN_SPEEDS` | `data[0..2]`=CPU LE16，`data[2..4]`=GPU LE16，`data[6..8]`=SYS LE16 |
  | 16 | `RGB_KB_MODE` | 0 off / 1 cyclic / 2 fixed / 3 custom |
  | 17 | `RGB_KB_COLOR` | |
  | 18 | `RGB_KB_BRIGHTNESS` | 0..3 |
  | 19 | `SYSTEM_AC_TYPE` | 1 = TypeC 供电 |
  | 20 | `MAX_FAN_SWITCH` | `payload[0]=0`(CPU/GPU)、`payload[1]=bool` |
  | 21 | `MAX_FAN_SPEED` | PWM 占空比 |
  | 22 | `CPU_THERMOMETER` | `data[0]` = 摄氏度 |
  | 23 | `CPU_POWER` | |
- 用户态可见路径（驱动加载后）：
  - hwmon chip 名 `tongfang_mifs`：`temp1_input`(m°C)、`fan1_input`(CPU)、`fan2_input`(GPU)、`fan3_input`(SYS)、`fan{1,2,3}_label`
  - WMI 设备目录下：`gpu_mode`(rw, `hybrid|discrete|uma`)、`kb_mode`(rw, `off|cyclic|fixed|custom`)、`fan_boost`(wo, bool)
  - 键盘背光 LED：`/sys/class/leds/laptop::kbd_backlight/brightness`（max 3）
  - 性能模式：标准 `platform_profile`（`LOW_POWER/BALANCED/BALANCED_PERFORMANCE/PERFORMANCE`），`/sys/firmware/acpi/platform_profile`
  - 热键 input 设备名 `Tongfang MIFS WMI hotkeys`（keymap 只映射 `KEY_PROG1/KEY_CALC/KEY_WWW`，其余 `KE_IGNORE`）
- 事件结构：`{u8 event_type; u8 event_id; u8 value_low; u8 value_high; u8 reserved[4];}`，`event_type=1` 为热键。事件 id 关键值：4 飞行模式、6 触摸板、9 CapsLock、15 性能模式变更、26 CPU 风扇转速、32 GPU 风扇转速。
- **对 OpenRevo 的影响**：`hal` 层做一个 `TongfangWmiBackend`（读 hwmon + 写 sysfs）作为首选，`ec_sys` 作为 fallback（驱动缺席时）。两者都实现 `HardwareDriver`，`hardware_probe` 选路。裸 EC 写入仍是最后手段，且必须先 `sudo modprobe ec_sys write_support=1`。

**实测本机**：当前开发机是 VirtualBox（`sys_vendor=innotek GmbH`，`product_name=VirtualBox`，kernel `7.3.5-zen1-1-zen`），`/sys/bus/wmi/devices` 为空、无 EC、无该模块——**所有硬件相关验证必须在真机做**，本机只能验证编译与无硬件降级路径。

## 2. userspace 参考：gglib（Tauri + Wayland 的真实做法）

仓库 `github.com/mmogr/gglib`，`src-tauri/src/tray/linux.rs` 与 `src-tauri/src/tray/layer_shell.rs`，存档 `ref/gglib-tray-linux.rs`、`ref/gglib-layer-shell.rs`。

### 2.1 托盘：用 `ksni` 而不是 Tauri 的 `tray-icon`
原文注释（`tray/linux.rs` 抬头）：

> Tauri's tray goes through `libappindicator`, which registers its item with `ItemIsMenu` and delivers no click events at all: `on_tray_icon_event` never fires and `TrayIcon::rect` is always `None`.

**借什么**：OpenRevo 现在点击托盘要弹主界面/OSD，依赖点击事件与图标坐标。Linux 上改用 `ksni`（`ksni::{Tray, TrayMethods, Handle, Icon, MenuItem}`），`Activate` 回调自带屏幕坐标，可直接喂给 layer-shell 定位。

### 2.2 浮层定位：`libloading` 动态加载 `libgtk-layer-shell.so.0`
关键常量与调用序列（`tray/layer_shell.rs`）：
```rust
const LAYER_OVERLAY: c_int = 3;
const EDGE_RIGHT: c_int = 1;
const EDGE_BOTTOM: c_int = 3;
const KEYBOARD_ON_DEMAND: c_int = 2;
// 符号：gtk_layer_is_supported / gtk_layer_init_for_window / gtk_layer_set_layer
//       gtk_layer_set_anchor / gtk_layer_set_margin / gtk_layer_set_keyboard_mode
```
- 必须**动态加载**：硬链接会让没装库的用户无法启动。
- `gtk_layer_init_for_window` 必须在窗口 realize 之前、GTK 主线程上调用（Tauri `setup` 里做）。
- `gtk_layer_is_supported()` 返回 0 时（Mutter 不实现 `zwlr_layer_shell_v1`，X11 也没有）必须优雅退回。
- 窗口指针转换：`GtkApplicationWindow*` 直接 `as` 成 `GtkWindow*`（GObject 父结构在前）。

**借什么**：OpenRevo 的 OSD / 桌面小组件窗口在 KDE Wayland 上用同一套。

### 2.3 其余可借点
- `tauri-plugin-single-instance`、`tauri-plugin-autostart` 已在 gglib 里工作于 Linux，可直接用于替换仓库里自写的 `system/autostart.rs`（后者 8 个 error 全来自注册表 API）。
- **主窗口不要用 layer-shell**：layer surface 不可自由缩放，用户要的"主界面自由调整大小"必须走普通 toplevel。

## 3. Tauri v2 Linux 依赖（Arch 官方清单）

来源 `v2.tauri.app/start/prerequisites/`（存档 `ref/tauri-prerequisites.mdx`）：

```
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl \
                        appmenu-gtk-module libappindicator-gtk3 librsvg xdotool
```

本次已补装 `base-devel openssl appmenu-gtk-module xdotool`（其余先前已装）。

## 4. Wayland 全局快捷键

- `org.freedesktop.portal.GlobalShortcuts`（XDG portal）。参考实现：Python `global-shortcut-portal`（pypi）、Go `AshBuk/go-wlportal`。
- 约束：需要 portal 后端支持（KDE 的 `xdg-desktop-portal-kde` 已实现）；快捷键需用户确认一次，绑定跨会话持久。
- 对 OpenRevo 的意义：Windows 上全局热键走 `RegisterHotKey`，Linux 只能走 portal，且**不能**保证独占组合键。

## 5. Slint 在 Wayland 上的现状

- `zwlr_layer_shell_v1` 支持仍在讨论中（slint-ui/slint Discussion #1765），**库本身不提供 layer-shell**。
- 可行路径：`slint = { features = ["unstable-winit-030"] }` → `WinitWindowAccessor::winit_window()` 拿到 `&winit::window::Window` → 通过 `raw_window_handle` 取 `wl_surface`/`xdg_toplevel`，自己接 `wayland-protocols` 的 layer-shell。
- 更省事的路径：主界面用普通 toplevel（可缩放，系统/自绘标题栏），只有 OSD 与小组件需要浮层时再上 layer-shell。
- 透明与圆角：Windows 走 DWM（`DwmExtendFrameIntoClientArea`、`DWMWA_WINDOW_CORNER_PREFERENCE`）；Wayland 上 `no-frame: true` + `background: transparent` 直接生效，但**没有服务端装饰就没有可拖拽的边框**，自由缩放要自己实现 CSD 或允许系统装饰。

## 6. 硬件抽象参考（非本机同型号，看结构）

- AUR `mechrevo-drivers-dkms-git`（4.24.0.r30.g85bf136-1）、`tuxedo-drivers-xmg-dkms-git`：整包 DKMS 驱动集合，可读其 EC 地址表交叉验证 `hal/vault.rs`。
- `clevo-keyboard`（TUXEDO）：键盘背光/热键的 EC 访问范例，ITE EC 用法与同方不同，仅借结构。
