; Windows installer (Inno Setup 6). Per-user: no administrator rights needed.
;
; Built by the release workflow:
;   iscc packaging\windows\installer.iss
; with APP_NAME, APP_ID, APP_VERSION (from `cargo xtask brand`), STAGE_DIR
; (from stage.ps1) and OUT_DIR in the environment.

#define AppName GetEnv("APP_NAME")
#define AppId GetEnv("APP_ID")
#define AppVersion GetEnv("APP_VERSION")
#define Stage GetEnv("STAGE_DIR")
#define OutDir GetEnv("OUT_DIR")

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppName}
AppPublisherURL=https://github.com/Rhyslos/Kiria-Video-Editor
DefaultDirName={localappdata}\Programs\{#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
OutputDir={#OutDir}
OutputBaseFilename={#AppName}-Setup-{#AppVersion}
SetupIconFile=..\icons\icon.ico
UninstallDisplayIcon={app}\launcher.exe
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#Stage}\launcher.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\current"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\versions\*"; DestDir: "{app}\versions"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
; Shortcuts always open the launcher, never the editor directly.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\launcher.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\launcher.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\launcher.exe"; Description: "Start {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Versions installed later by the updater are removed too. Projects and settings are never touched.
Type: filesandordirs; Name: "{app}\versions"
Type: files; Name: "{app}\current"
Type: files; Name: "{app}\current.new"
Type: files; Name: "{app}\launcher.old.exe"
