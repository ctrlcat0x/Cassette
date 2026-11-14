#ifndef AppVersion
  #error AppVersion must match the version in Cargo.toml
#endif

[Setup]
AppId=app.raag.desktop
AppName=Cassette
AppVersion={#AppVersion}
AppPublisher=Cassette
AppPublisherURL=https://github.com/ctrlcat0x/Cassette
DefaultDirName={localappdata}\Programs\Cassette
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\cassette.exe
SetupIconFile=..\assets\cassette.ico
OutputDir=..\dist
OutputBaseFilename=cassette-windows-x64-setup
Compression=lzma2
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "startmenu"; Description: "Create a Start menu shortcut"
Name: "desktop"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\cassette.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Cassette"; Filename: "{app}\cassette.exe"; WorkingDir: "{app}"; Tasks: startmenu
Name: "{userdesktop}\Cassette"; Filename: "{app}\cassette.exe"; WorkingDir: "{app}"; Tasks: desktop

[Run]
Filename: "{app}\cassette.exe"; Description: "Launch Cassette"; Flags: nowait postinstall skipifsilent
