; Inno Setup script for Beez Video Studio.
;
; Build (after `cargo build --release`):
;   ISCC.exe /DAppVersion=0.1.0 installer\beezvideo.iss
; Output: dist\Beez-Video-Studio-Setup-v<version>[-Standalone].exe
;
; Installs per user into %LOCALAPPDATA%\Programs (no administrator rights needed). This also
; keeps the in-app self-updater working, because the app can replace its own .exe there.
;
; Standalone build: put ffmpeg.exe and ffprobe.exe into installer\ffmpeg\ before compiling.
; They are installed into {app}\ffmpeg\bin (where the app looks first), so the app works
; offline right away, and the output is named ...-Standalone.exe. Without them, the app
; offers a one-click FFmpeg download on first start.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "Beez Video Studio"
#define AppExe "beez_video_studio.exe"
#define AppUrl "https://github.com/smbhathiya/beez_video_studio"

#if FileExists(AddBackslash(SourcePath) + "ffmpeg\ffmpeg.exe") && FileExists(AddBackslash(SourcePath) + "ffmpeg\ffprobe.exe")
  #define Bundled
  #define Flavor "-Standalone"
#else
  #define Flavor ""
#endif

[Setup]
; Never change AppId: Windows uses it to recognise upgrades of the same app.
AppId={{40899279-F498-478A-9080-E88728232ED2}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Beez
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; 64-bit Windows 10/11 (Windows 11 on ARM runs it through x64 emulation).
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Beez-Video-Studio-Setup-v{#AppVersion}{#Flavor}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "openwith"; Description: "Show {#AppName} in the ""Open with"" menu for video files"; GroupDescription: "Integration:"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "README.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme
#ifdef Bundled
Source: "ffmpeg\ffmpeg.exe"; DestDir: "{app}\ffmpeg\bin"; Flags: ignoreversion
Source: "ffmpeg\ffprobe.exe"; DestDir: "{app}\ffmpeg\bin"; Flags: ignoreversion
#endif

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; "Open with" entries for common video types (per user; removed on uninstall).
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".mov"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".mkv"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".webm"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".avi"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.mp4\OpenWithList\{#AppExe}"; ValueType: string; ValueData: ""; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.mov\OpenWithList\{#AppExe}"; ValueType: string; ValueData: ""; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.mkv\OpenWithList\{#AppExe}"; ValueType: string; ValueData: ""; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.webm\OpenWithList\{#AppExe}"; ValueType: string; ValueData: ""; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.avi\OpenWithList\{#AppExe}"; ValueType: string; ValueData: ""; Flags: uninsdeletekey; Tasks: openwith

[UninstallDelete]
; An FFmpeg copy the app downloaded itself, and its settings and log.
Type: filesandordirs; Name: "{localappdata}\{#AppName}"

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
