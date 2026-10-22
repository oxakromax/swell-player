; Inno Setup 6 script for Swell on Windows (x64).
;
; Packs the folder made by packaging/windows/bundle.sh:
;   iscc /DAppVersion=1.3.1 /DSourceDir=C:\...\dist\windows\Swell /DOutputDir=C:\...\dist packaging\windows\swell.iss
;
; Installs per user without administrator rights by default; the first page
; offers an all-users install instead (which Swell only tells about new
; releases, see src/update/channel.rs).
;
; Swell updates itself by running a newer installer silently when it quits
; (src/update/install.rs):
;   setup.exe /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /NORESTARTAPPLICATIONS /relaunch=0|1 /swellpid=<pid>
; It waits for that Swell, and for every program in the install, to exit
; before it changes anything (see [Code]); a Swell that still runs (it
; holds the AppMutex) stops it. With /relaunch=1 (the user chose Restart)
; Swell starts again afterwards, also when the update fails.

#ifndef AppVersion
  #define AppVersion "1.3.1"
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
; One Setup at a time: two Swell instances may quit with the same update.
SetupMutex=SwellPlayerSetup,Global\SwellPlayerSetup
; Held by every running Swell (src/main.rs).
AppMutex=SwellPlayerRunning

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
; An update that Swell started for a restart: start it again, as the user
; who ran it (not as the administrator an all-users install elevates to).
Filename: "{app}\{#AppExe}"; Flags: nowait runasoriginaluser; Check: RelaunchAfterUpdate

[Code]
const
  SYNCHRONIZE = $00100000;

var
  { The install folder, once known, and whether the files went in. }
  AppDir: String;
  Installed: Boolean;

function OpenProcess(DesiredAccess: DWORD; InheritHandle: BOOL; ProcessId: DWORD): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(Handle: THandle; Milliseconds: DWORD): DWORD;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

function RelaunchAfterUpdate: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;

{ The process id of the Swell that started this update as it quit, or 0. }
function SwellPid: Integer;
begin
  Result := StrToIntDef(ExpandConstant('{param:swellpid|0}'), 0);
end;

{ Waits (up to 30 s) for that Swell to exit before Setup checks the
  AppMutex, which it holds until then. }
function InitializeSetup: Boolean;
var
  Process: THandle;
begin
  if SwellPid <> 0 then begin
    Process := OpenProcess(SYNCHRONIZE, False, SwellPid);
    if Process <> 0 then begin
      Log('Waiting for Swell to exit');
      WaitForSingleObject(Process, 30000);
      CloseHandle(Process);
    end;
  end;
  Result := True;
end;

{ Whether a program in Dir runs: a running program cannot be opened for
  writing. }
function ProgramRunsIn(const Dir: String): Boolean;
var
  FindRec: TFindRec;
  Stream: TFileStream;
begin
  Result := False;
  if FindFirst(Dir + '\*.exe', FindRec) then
    try
      repeat
        try
          Stream := TFileStream.Create(Dir + '\' + FindRec.Name, fmOpenReadWrite or fmShareExclusive);
          Stream.Free;
        except
          Result := True;
        end;
      until Result or not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
end;

{ An update also waits (up to 30 s) for the programs Swell started to exit:
  gdbus.exe, the session bus GApplication starts, stays a few seconds after
  the last client leaves. Setup cannot replace a running program or the DLLs
  it loaded, and would leave a half-replaced install that no longer starts;
  returning a message stops it before it changes anything. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Tries: Integer;
begin
  Result := '';
  AppDir := ExpandConstant('{app}');
  if SwellPid = 0 then
    Exit;
  Tries := 0;
  while ProgramRunsIn(AppDir + '\bin') or ProgramRunsIn(AppDir + '\libexec\gstreamer-1.0') do begin
    Tries := Tries + 1;
    if Tries > 60 then begin
      Result := 'Swell, or a program it started, still runs from ' + AppDir;
      Exit;
    end;
    Sleep(500);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    Installed := True;
end;

{ An update for a restart that stopped early or failed starts the Swell that
  is there, so that it does not just vanish. }
procedure DeinitializeSetup;
var
  ResultCode: Integer;
begin
  if RelaunchAfterUpdate and not Installed and (AppDir <> '')
    and FileExists(AppDir + '\{#AppExe}') then
    ExecAsOriginalUser(AppDir + '\{#AppExe}', '', '', SW_SHOWNORMAL, ewNoWait, ResultCode);
end;
