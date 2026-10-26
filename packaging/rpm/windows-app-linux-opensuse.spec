Name:           windows-app-linux
Version:        0.1.0
Release:        1%{?dist}
Summary:        Microsoft Windows 365 Cloud PC and Azure Virtual Desktop client for Linux
License:        Apache-2.0

Requires:       gtk4 >= 4.14
Requires:       libadwaita-1-0 >= 1.5
Requires:       libwebkit2gtk-6_0-4 >= 2.42
Requires:       libsoup-3_0-0 >= 3.0
Requires:       freerdp >= 3.0

%description
Native Linux client written in Rust using GTK 4 and WebKitGTK 6 for connecting
to Microsoft Windows 365 Cloud PCs and Azure Virtual Desktop (AVD).

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/scalable/apps

install -m 755 %{_sourcedir}/windows-app-linux %{buildroot}%{_bindir}/windows-app-linux
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
