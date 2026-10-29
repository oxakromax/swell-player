; Inno Setup 6 script for Swell on Windows (x64).
;
; Packs the folder made by packaging/windows/bundle.sh:
;   iscc /DAppVersion=1.3.3 /DSourceDir=C:\...\dist\windows\Swell /DOutputDir=C:\...\dist packaging\windows\swell.iss
;
; Installs per user without administrator rights by default; the first page
; offers an all-users install instead (which Swell only tells about new
; releases, see src/update/channel.rs).
;
; Swell updates itself by running a newer installer silently when it quits
; (src/update/install.rs):
;   setup.exe /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /NORESTARTAPPLICATIONS /relaunch=0|1 /swellpid=<pid>
; It waits for that Swell, and for every program in the install, to exit
; before it changes anything, and ends what outlives it (see [Code] and
; end-helpers.ps1); another Swell that still runs (it holds the AppMutex)
; stops it. With /relaunch=1 (the user chose Restart) Swell starts again
; afterwards, also when the update fails. The log (/LOG=) says what it
; waited for and ended.

#ifndef AppVersion
  #define AppVersion "1.3.3"
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
; Run from {tmp} by an update (see EndLeftovers).
Source: "end-helpers.ps1"; Flags: dontcopy

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
  PROCESS_TERMINATE = $0001;
  PROCESS_QUERY_LIMITED_INFORMATION = $1000;
  WAIT_TIMEOUT = $00000102;

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
function TerminateProcess(Handle: THandle; ExitCode: Cardinal): BOOL;
  external 'TerminateProcess@kernel32.dll stdcall';
function QueryFullProcessImageName(Handle: THandle; Flags: DWORD; ExeName: String; var Size: DWORD): BOOL;
  external 'QueryFullProcessImageNameW@kernel32.dll stdcall';

function RelaunchAfterUpdate: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;

{ The process id of the Swell that started this update as it quit, or 0. }
function SwellPid: Integer;
begin
  Result := StrToIntDef(ExpandConstant('{param:swellpid|0}'), 0);
end;

{ The Swell that started this update still runs 30 s after it quit: it has
  nothing left to do, but hangs on its way out, holding the AppMutex. Ends
  it, if the process is still a Swell. }
procedure EndHungSwell(Process: THandle);
var
  Path: String;
  Size: DWORD;
begin
  Size := 1024;
  Path := StringOfChar(#0, 1024);
  if QueryFullProcessImageName(Process, 0, Path, Size) then begin
    SetLength(Path, Size);
    if CompareText(ExtractFileName(Path), 'swell-player.exe') = 0 then begin
      Log('Swell still runs after 30 s; ending ' + Path);
      if TerminateProcess(Process, 1) then
        WaitForSingleObject(Process, 5000)
      else
        Log('Could not end it: ' + SysErrorMessage(DLLGetLastError));
      Exit;
    end;
  end;
  Log('Process ' + IntToStr(SwellPid) + ' still runs after 30 s');
end;

{ Waits (up to 30 s) for that Swell to exit before Setup checks the
  AppMutex, which it holds until then. }
function InitializeSetup: Boolean;
var
  Process: THandle;
begin
  if SwellPid <> 0 then begin
    Process := OpenProcess(SYNCHRONIZE or PROCESS_TERMINATE or PROCESS_QUERY_LIMITED_INFORMATION, False, SwellPid);
    if Process = 0 then
      Process := OpenProcess(SYNCHRONIZE, False, SwellPid);
    if Process <> 0 then begin
      Log('Waiting for Swell to exit');
      if WaitForSingleObject(Process, 30000) = WAIT_TIMEOUT then
        EndHungSwell(Process);
      CloseHandle(Process);
    end;
  end;
  Result := True;
end;

{ The first program in Dir that runs (a running program cannot be opened
  for writing), or ''. }
function ProgramRunningIn(const Dir: String): String;
var
  FindRec: TFindRec;
  Stream: TFileStream;
begin
  Result := '';
  if FindFirst(Dir + '\*.exe', FindRec) then
    try
      repeat
        try
          Stream := TFileStream.Create(Dir + '\' + FindRec.Name, fmOpenReadWrite or fmShareExclusive);
          Stream.Free;
        except
          Result := Dir + '\' + FindRec.Name;
        end;
      until (Result <> '') or not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
end;

{ A program of the install that runs, or ''. }
function RunningProgram: String;
begin
  Result := ProgramRunningIn(AppDir + '\bin');
  if Result = '' then
    Result := ProgramRunningIn(AppDir + '\libexec\gstreamer-1.0');
end;

{ Ends the programs of the install that outlived the Swell that started
  this update (end-helpers.ps1 says which, in the log). }
procedure EndLeftovers;
var
  ResultCode: Integer;
begin
  ExtractTemporaryFile('end-helpers.ps1');
  if not ExecAndLogOutput(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + ExpandConstant('{tmp}\end-helpers.ps1') +
    '" -AppDir "' + AppDir + '" -SwellPid ' + IntToStr(SwellPid),
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode, nil) then
    Log('Could not run end-helpers.ps1: ' + SysErrorMessage(ResultCode));
end;

{ An update also waits (up to 30 s) for the programs Swell started to exit,
  and after 10 s ends those that outlived it: gdbus.exe, the session bus
  GApplication starts, leaves 3 s after its last client, but never when
  none ever connected. Setup cannot replace a running program or the DLLs
  it loaded, and would leave a half-replaced install that no longer starts;
  returning a message stops it before it changes anything. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Tries: Integer;
  Running: String;
begin
  Result := '';
  AppDir := ExpandConstant('{app}');
  if SwellPid = 0 then
    Exit;
  Tries := 0;
  Running := RunningProgram;
  while Running <> '' do begin
    if Tries = 0 then
      Log('Waiting for ' + Running + ' to exit');
    if Tries = 20 then
      EndLeftovers;
    if Tries >= 60 then begin
      Result := 'Swell, or a program it started, still runs: ' + Running;
      Log(Result);
      Exit;
    end;
    Sleep(500);
    Tries := Tries + 1;
    Running := RunningProgram;
  end;
  if Tries > 0 then
    Log(Format('Every program of the install exited after %d ms', [Tries * 500]));
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
