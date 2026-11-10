; Per-user Windows installer for the desktop app, built by .github/workflows/desktop-release.yml:
;   ISCC /DStageDir=<staged folder> /DOutputDir=<output folder> /DAppVersion=<X.Y.Z>
;        /DIconPath=<icon.ico> knowledge-garden.iss
; Keep the AppId so later installers upgrade the same installation. The app updates itself in
; place afterwards (desktop/src/update.rs); installing a newer setup over it works too.
[Setup]
AppId={{03A94087-7841-4B53-8488-52872CFDCD56}
AppName=Knowledge Garden
AppVersion={#AppVersion}
AppPublisher=uthree
AppPublisherURL=https://github.com/uthree/knowledge-garden
AppSupportURL=https://github.com/uthree/knowledge-garden/issues
DefaultDirName={localappdata}\Programs\Knowledge Garden
DefaultGroupName=Knowledge Garden
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
ShowLanguageDialog=auto
SetupIconFile={#IconPath}
UninstallDisplayIcon={app}\knowledge-garden.ico
OutputDir={#OutputDir}
OutputBaseFilename=knowledge-garden-desktop-v{#AppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
CloseApplications=yes

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Icons]
Name: "{autoprograms}\Knowledge Garden"; Filename: "{app}\knowledge-garden-desktop.exe"; IconFilename: "{app}\knowledge-garden.ico"
Name: "{userdesktop}\Knowledge Garden"; Filename: "{app}\knowledge-garden-desktop.exe"; IconFilename: "{app}\knowledge-garden.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\knowledge-garden-desktop.exe"; Description: "{cm:LaunchProgram,Knowledge Garden}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; What the app's own updates leave beside it.
Type: files; Name: "{app}\knowledge-garden-desktop.exe.old"
Type: filesandordirs; Name: "{app}\.knowledge-garden-update"
