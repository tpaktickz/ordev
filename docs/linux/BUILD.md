# Building OpenRevo on Linux

This is the Linux port. The Windows build still lives in `src-tauri/tauri.conf.json`
and is unchanged; nothing here is Windows-only conditional.

## Package dependencies

Everything is from the distribution, no vendored copies:

```sh
sudo pacman -S --needed base-devel rust cargo nodejs pnpm \
  webkit2gtk-4.1 gtk3 librsvg libappindicator-gtk3 hicolor-icon-theme openssl
```

`webkit2gtk-4.1` is the Tauri runtime and the largest single dependency. The
native Slint companion windows do not use it, but the plugin host does, so it
stays a hard requirement.

Optional runtime tools, installed only if the desktop leans on them:

| tool | used for | absent means |
| :--- | :--- | :--- |
| `zenity` or `kdialog` | Import/save dialogs (colour profiles, fan curves) | dialogs return None |
| `wlr-randr` | Refresh-rate and rotation switches on wlroots | falls back to `kscreen-doctor`, then `xrandr` |
| `bsdtar` or `unzip` | Plugin package import | import fails with the missing binary named |
| `wpctl` or `pactl` | Microphone mute switch | switch reports unknown |
| `powerprofilesctl` | Power mode when firmware has no `platform_profile` | mode falls back to EC only |
| `lspci` | GPU model names | GPU shows as `PCI <vendor>:<device>` |

## Build

```sh
pnpm install --frozen-lockfile
pnpm build                    # Vite shell -> dist/
cd src-tauri
cargo build --release --bin open-revo
```

`open-revo` is a single binary that dispatches on argv: `--slint-main`,
`--osd-companion` and `--slint-widget` re-enter the same executable. The
per-crate `openrevo-main` / `openrevo-osd` / `openrevo-widget` binaries exist for
standalone development only and are not needed in the package.

For the packaged path use `packaging/linux/PKGBUILD`:

```sh
cd packaging/linux
makepkg -si
```

## Packaging for other distributions

Three formats are maintained, one per distribution, and all three build the same
tree into the same file layout. Each lives next to the one it belongs to:

| distribution | file | command |
| :--- | :--- | :--- |
| Arch | `packaging/linux/PKGBUILD` | `cd packaging/linux && makepkg -si` |
| Debian, Ubuntu | `packaging/debian/` | `dpkg-buildpackage -b -us -uc -d` |
| Fedora, RHEL | `packaging/rpm/openrevo.spec` | `rpmbuild -ba --define "_topdir $PWD/rpmbuild" --define "version X" packaging/rpm/openrevo.spec` |

`packaging/debian/` and `packaging/rpm/` expect `debian/` at the top of a source
tree or a `openrevo-<version>.tar.gz` in `SOURCES`, so both CI jobs export the
tree with `git archive` first instead of building in place. The Debian changelog
version is generated from `src-tauri/Cargo.toml` for the same reason: two copies
of the version drift.

Debian and Fedora ship a rustc below the dependency tree's floor (slint and wgpu
both sit above 1.80), so both jobs take the toolchain from rustup and link it into
`/usr/local/bin`; Arch's `rust` package is current and is used as packaged.

The helper binary installs to `/usr/lib/openrevo/openrevo-ec-helper` in all three
packages rather than `%{_libdir}`, because the polkit action pins that absolute
path in `com.openrevo.ecwrite.policy` and the caller hardcodes it.

`packaging/linux/openrevo.desktop` is the single desktop entry for all three;
only its installed name differs.

## Permissions

Three device classes are separate privileges, and the app degrades rather than
fails without them — see `packaging/linux/60-openrevo.rules` for the full matrix
and the `ec_sys` module parameter. Summary:

- `input` group → hotkeys and the idle clock (`/dev/input/event*`, passive read).
- root or `CAP_SYS_RAWIO` → EC access (`ec_sys` debugfs and the ECMD/ECDT ports).
- `uaccess` (automatic for the seat) → hidraw keyboard lighting, backlight.

Without any of the three grants, EC writes degrade: bank 0 reads still work when
`ec_sys` is loaded read-only, but nothing on the EC gets written and the app falls
back to `platform_profile` for power modes.

Two ways to get the port path without running the whole daemon as root:

- `sudo setcap cap_sys_rawio+ep /usr/bin/open-revo` — covers the EC ports, not
  debugfs (`ec_sys` nodes stay 0700 root).
- **Polkit helper** (opt-in): `OPENREVO_EC_HELPER=1 open-revo`. Every write that
  the in-process path cannot do is handed to `openrevo-ec-helper` through
  `pkexec`; `com.openrevo.ecwrite` grants `auth_admin_keep`, so a mode switch is
  one prompt and stays quiet for a while afterwards. Off by default because a
  prompt during boot is intrusive and the kernel module covers most machines.
  The helper accepts only bounded `w <addr16> <value8>` lines (max 64) and
  exposes no other privileged operation.

## Kernel driver (preferred)

Load the Tongfang MIFS WMI module if your kernel has it, because it takes the EC
mailbox away from userspace and exposes fan RPM, platform profile, GPU mode and
keyboard LED as sysfs nodes:

```sh
sudo modprobe ec_sys write_support=1        # fallback EC channel
# and/or
sudo modprobe tongfang-mifs-wmi             # preferred channel, if built
```

The driver is not in mainline. Build it out of tree from the patch series in
`docs/linux/ref/`, or install one of the AUR packages that carry it
(`mechrevo-drivers-dkms-git`, `tuxedo-drivers-xmg-dkms-git`).

Order of preference, as implemented in `hal/driver_impl.rs`:

1. `tongfang-mifs-wmi` sysfs (`pal::tongfang`) — kernel owns the mailbox.
2. Raw EC via `ec_sys` debugfs / ECMD-ECDT ports (`pal::ec`).
3. `MockHAL`, which keeps the UI alive with synthetic values.

## Autostart: two mechanisms, pick one

The app can register itself through the XDG autostart spec (Settings → Startup),
which is the portable path. The package also ships
`/usr/lib/systemd/user/openrevo.service` for sessions that prefer a user unit.
Enabling both starts two instances; the single-instance guard folds the second
into the first, so the cost is only a wasted start, but pick one.

```sh
systemctl --user enable --now openrevo.service   # systemd route
```

## Running without hardware

`MOCK_HAL=1` is not needed: on a machine with no EC channel the probe logs
`No physical hardware driver matched. Activating MockHAL backend.` and the UI
still renders.

## Verifying a build headlessly

Slint needs a compositor even when there is no physical display:

```sh
export XDG_RUNTIME_DIR=/tmp/xdg-wl WAYLAND_DISPLAY=wl-test
weston --backend=headless-backend.so --socket=wl-test --width=1920 --height=1080 &
./target/debug/open-revo --slint-main --visible
```

Expect `set_min_size(320, 480)`, `set_max_size(0, 0)` and a
`set_window_geometry` matching 395x632 or 1120x760 in `WAYLAND_DEBUG=1` output.
