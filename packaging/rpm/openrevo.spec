Name:           openrevo
# CI passes the real version in with --define; the literal is a fallback so the
# spec can still be inspected on its own.
Version:        %{?version}%{!?version:0.9.8}
Release:        1%{?dist}
Summary:        Control center for Mechrevo and Tongfang laptops
License:        GPL-3.0-or-later
URL:            https://github.com/faintonce/open-revo
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  nodejs
BuildRequires:  npm
BuildRequires:  pkgconf-pkg-config
BuildRequires:  webkit2gtk4.1-devel
BuildRequires:  gtk3-devel
BuildRequires:  librsvg2-devel
BuildRequires:  libappindicator-gtk3-devel
BuildRequires:  openssl-devel
BuildRequires:  dbus-devel
BuildRequires:  systemd-devel
BuildRequires:  libsoup3-devel
BuildRequires:  javascriptcoregtk4.1-devel
BuildRequires:  zlib-devel
BuildRequires:  systemd-rpm-macros

Requires:       webkit2gtk4.1
Requires:       gtk3
Requires:       librsvg2
Requires:       libappindicator-gtk3
Requires:       hicolor-icon-theme
Requires:       polkit

# Both macros normally arrive with systemd-rpm-macros; the fallbacks keep the
# paths meaningful when the spec is parsed on a system without them.
%{!?_udevrulesdir:%global _udevrulesdir %{_prefix}/lib/udev/rules.d}
%{!?_userunitdir:%global _userunitdir %{_prefix}/lib/systemd/user}

%description
OpenRevo drives the EC, keyboard lighting, fan curves, pause and battery limits
of Mechrevo/Tongfang laptops through the sysfs kernel interfaces
(tongfang-mifs-wmi hwmon, platform_profile, the kbd_backlight LED) and falls back
to direct EC access through ec_sys and the ACPI ports.

This build targets Wayland and X11 desktops and only uses freedesktop services:
StatusNotifier for the tray, logind for sleep events, polkit for the privileged
EC fallback, and the XDG base directories.

%prep
%autosetup -n %{name}-%{version}

%build
export CARGO_TARGET_DIR=%{_builddir}/%{name}-%{version}/target
npm i -g pnpm@10.32.1
pnpm install --frozen-lockfile
pnpm build
cd src-tauri
cargo build --locked --release --bin open-revo
# The helper belongs to openrevo-core: --bin alone is scoped to the default-run
# package and aborts with "no bin target named openrevo-ec-helper".
cargo build --locked --release -p openrevo-core --bin openrevo-ec-helper

%install
# %build and %install are separate scripts, so the target dir is recomputed here
# rather than inherited from the environment.
TARGET=%{_builddir}/%{name}-%{version}/target
install -Dm755 $TARGET/release/open-revo %{buildroot}%{_bindir}/open-revo
install -Dm644 packaging/linux/openrevo.desktop %{buildroot}%{_datadir}/applications/open-revo.desktop
for size in 32 64 128; do
  install -Dm644 "src-tauri/icons/${size}x${size}.png" \
    %{buildroot}%{_datadir}/icons/hicolor/${size}x${size}/apps/open-revo.png
done
install -Dm644 src-tauri/icons/128x128@2x.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/open-revo.png

# udev rules, the ec_sys module parameter and the modules-load file are one
# unit: the rule file alone leaves every bank-0 write silently failing.
install -Dm644 packaging/linux/60-openrevo.rules %{buildroot}%{_udevrulesdir}/60-openrevo.rules
install -Dm644 packaging/linux/modprobe-openrevo.conf %{buildroot}%{_sysconfdir}/modprobe.d/openrevo.conf
install -Dm644 packaging/linux/modules-load-openrevo.conf %{buildroot}%{_sysconfdir}/modules-load.d/openrevo.conf
install -Dm644 packaging/linux/openrevo.service %{buildroot}%{_userunitdir}/openrevo.service

# Not setuid: pkexec plus the policy decides, and the policy pins this path.
install -Dm755 $TARGET/release/openrevo-ec-helper %{buildroot}%{_prefix}/lib/openrevo/openrevo-ec-helper
install -Dm644 packaging/linux/com.openrevo.ecwrite.policy \
  %{buildroot}%{_datadir}/polkit-1/actions/com.openrevo.ecwrite.policy

%files
%license LICENSE
%{_bindir}/open-revo
%{_datadir}/applications/open-revo.desktop
%{_datadir}/icons/hicolor/*/apps/open-revo.png
%{_udevrulesdir}/60-openrevo.rules
%config %{_sysconfdir}/modprobe.d/openrevo.conf
%config %{_sysconfdir}/modules-load.d/openrevo.conf
%{_userunitdir}/openrevo.service
%{_prefix}/lib/openrevo/openrevo-ec-helper
%{_datadir}/polkit-1/actions/com.openrevo.ecwrite.policy

%changelog
* Tue Nov 24 2026 openrevo packagers <packagers@openrevo.invalid> - 0.9.8-1
- Initial Linux package (Wayland/X11 port).
