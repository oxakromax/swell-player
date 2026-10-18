; Inno Setup 6 script for Swell on Windows (x64).
;
; Packs the folder made by packaging/windows/bundle.sh:
;   iscc /DAppVersion=1.1.0 /DSourceDir=C:\...\dist\windows\Swell /DOutputDir=C:\...\dist packaging\windows\swell.iss
;
; Installs per user without administrator rights by default; the first page
; offers an all-users install instead.

#ifndef AppVersion
  #define AppVersion "1.1.0"
#endif
#ifndef SourceDir
  #define SourceDir AddBackslash(SourcePath) + "..\..\dist\windows\Swell"
#endif
#ifndef OutputDir
  #define OutputDir AddBackslash(SourcePath) + "..\..\dist"
#endif

#define AppName "Swell"
#define AppId "io.github.oxakromax.SwellPlayer"
#define AppExe "bin\swell-player.exe"
#define AppUrl "https://github.com/oxakromax/swell-player"

[Setup]
; Never change AppId: it ties upgrades and the uninstaller together.
AppId={{0B3E9C1D-6D2A-4E8F-9C51-5A1F7E2B8D47}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=oxakromax
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
AppCopyright=Swell contributors, GPL-3.0-or-later
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
LicenseFile={#SourceDir}\COPYING.txt
SetupIconFile={#SourceDir}\swell.ico
UninstallDisplayIcon={app}\swell.ico
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=swell-player-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[CustomMessages]
english.TidalLinks=Open tidal:// links with Swell
spanish.TidalLinks=Abrir los enlaces tidal:// con Swell

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "tidallinks"; Description: "{cm:TidalLinks}"; Flags: unchecked

[InstallDelete]
; The bundle is replaced as a whole on upgrade, so stale DLLs and plugins
; from an older version cannot linger.
Type: filesandordirs; Name: "{app}\bin"
Type: filesandordirs; Name: "{app}\lib"
Type: filesandordirs; Name: "{app}\libexec"
Type: filesandordirs; Name: "{app}\share"
Type: filesandordirs; Name: "{app}\etc"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\swell.ico"; AppUserModelID: "{#AppId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\swell.ico"; AppUserModelID: "{#AppId}"; Tasks: desktopicon

[Registry]
; tidal:// URL scheme (HKA: the current user's or the machine's classes,
; matching the install mode).
Root: HKA; Subkey: "Software\Classes\tidal"; ValueType: string; ValueName: ""; ValueData: "URL:TIDAL link"; Flags: uninsdeletekey; Tasks: tidallinks
Root: HKA; Subkey: "Software\Classes\tidal"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: tidallinks
Root: HKA; Subkey: "Software\Classes\tidal\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\swell.ico"""; Tasks: tidallinks
Root: HKA; Subkey: "Software\Classes\tidal\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: tidallinks

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
