; Inno Setup Script for Windows App (Windows 365, AVD & Remote Desktop client)
; Generates a standalone, double-clickable installer: windows-app-windows-x64-setup.exe
; Bundles the application executable, FreeRDP 3 DLLs, Windows App SDK runtime, and Visual C++ CRT.

#ifndef AppVersion
#define AppVersion "0.2.0"
#endif

#define AppName "Windows App"
#define AppPublisher "Windows App Contributors"
#define AppURL "https://github.com/sudhirdhumal289/windows-app-linux"
#define AppExeName "WindowsApp.exe"

[Setup]
AppId={{C84FD9D8-22F1-4FCE-8F28-569A5D6E77A1}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
AllowNoIcons=yes
OutputDir=..\..\target\packages
OutputBaseFilename=windows-app-windows-x64-setup
SetupIconFile=AppIcon.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Non-admin install by default to %LOCALAPPDATA%\Programs\Windows App (no UAC prompt required),
; but allows elevating to All Users (Program Files) if chosen or run as admin.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "associaterdp"; Description: "Associate with Remote Desktop (.rdp) files"; GroupDescription: "File associations:"; Flags: unchecked

[Files]
; All staged files: WindowsApp.exe, FreeRDP DLLs, Windows App SDK runtime, CRT DLLs, resources
Source: "..\..\target\windows-package\Windows App\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\AppIcon.ico"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\AppIcon.ico"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.rdp\OpenWithProgids"; ValueType: string; ValueName: "WindowsApp.Rdp"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associaterdp
Root: HKA; Subkey: "Software\Classes\WindowsApp.Rdp"; ValueType: string; ValueName: ""; ValueData: "Remote Desktop Connection"; Flags: uninsdeletekey; Tasks: associaterdp
Root: HKA; Subkey: "Software\Classes\WindowsApp.Rdp\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName},0"; Tasks: associaterdp
Root: HKA; Subkey: "Software\Classes\WindowsApp.Rdp\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""; Tasks: associaterdp

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
