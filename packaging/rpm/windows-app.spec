Name:           windows-app
Version:        0.2.4
Release:        1%{?dist}
Summary:        Native client for Windows 365, Azure Virtual Desktop and Remote Desktop
License:        Apache-2.0 OR MIT
Provides:       windows-app-linux = %{version}-%{release}
Obsoletes:      windows-app-linux < %{version}-%{release}

Requires:       gtk4 >= 4.14
Requires:       libadwaita >= 1.5
Requires:       webkitgtk6.0 >= 2.42
Requires:       libsoup3 >= 3.0
Requires:       freerdp-libs >= 3.0

%description
Native client written in Rust using GTK 4 and WebKitGTK 6 for connecting
to Microsoft Windows 365 Cloud PCs, Azure Virtual Desktop (AVD), and Remote Desktop.

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/scalable/apps

install -m 755 %{_sourcedir}/windows-app %{buildroot}%{_bindir}/windows-app
ln -sf %{_bindir}/windows-app %{buildroot}%{_bindir}/windows-app-linux
install -m 644 %{_sourcedir}/com.microsoft.windowsapp.linux.desktop %{buildroot}%{_datadir}/applications/com.microsoft.windowsapp.linux.desktop
install -Dm 644 %{_sourcedir}/com.microsoft.windowsapp.linux.search-provider.ini %{buildroot}%{_datadir}/gnome-shell/search-providers/com.microsoft.windowsapp.linux.search-provider.ini
install -Dm 644 %{_sourcedir}/com.microsoft.windowsapp.linux.service %{buildroot}%{_datadir}/dbus-1/services/com.microsoft.windowsapp.linux.service
for file in metadata.json extension.js; do
    install -Dm 644 "%{_sourcedir}/gnome-shell-extension/windows-app-linux@sudhirdhumal289.github.io/${file}" \
        "%{buildroot}%{_datadir}/gnome-shell/extensions/windows-app-linux@sudhirdhumal289.github.io/${file}"
done
install -m 644 %{_sourcedir}/com.microsoft.windowsapp.linux.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/com.microsoft.windowsapp.linux.svg

for size in 16x16 24x24 32x32 48x48 64x64 128x128 256x256 512x512; do
    if [ -f "%{_sourcedir}/icons/${size}/apps/com.microsoft.windowsapp.linux.png" ]; then
        mkdir -p "%{buildroot}%{_datadir}/icons/hicolor/${size}/apps"
        install -m 644 "%{_sourcedir}/icons/${size}/apps/com.microsoft.windowsapp.linux.png" \
            "%{buildroot}%{_datadir}/icons/hicolor/${size}/apps/com.microsoft.windowsapp.linux.png"
    fi
done

%files
%{_bindir}/windows-app
%{_bindir}/windows-app-linux
%{_datadir}/applications/com.microsoft.windowsapp.linux.desktop
%{_datadir}/gnome-shell/search-providers/com.microsoft.windowsapp.linux.search-provider.ini
%{_datadir}/dbus-1/services/com.microsoft.windowsapp.linux.service
%{_datadir}/gnome-shell/extensions/windows-app-linux@sudhirdhumal289.github.io/
%{_datadir}/icons/hicolor/*/apps/com.microsoft.windowsapp.linux.*

%post
/usr/bin/update-desktop-database &> /dev/null || :
/usr/bin/gtk-update-icon-cache %{_datadir}/icons/hicolor &> /dev/null || :

%postun
/usr/bin/update-desktop-database &> /dev/null || :
/usr/bin/gtk-update-icon-cache %{_datadir}/icons/hicolor &> /dev/null || :
