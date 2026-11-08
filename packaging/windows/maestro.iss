; Inno Setup script for Maestro.
;
; BuildDir, VendorDir, Arch and Version come from build-installer.sh (ISCC /D);
; the directory paths are relative to this file. Companion<n>* are the same
; values for an architecture whose applications cannot load the native build —
; 32-bit hosts, and x64/x86 apps emulated on ARM64.
;
; Needs Inno Setup 6.3 or newer (the x64compatible architecture identifier).

#ifndef Version
  #error "Version is required: pass /DVersion=x.y.z"
#endif
#ifndef Arch
  #error "Arch is required: pass /DArch=x86_64|aarch64|x86"
#endif
#ifndef BuildDir
  #error "BuildDir is required"
#endif
#ifndef VendorDir
  #error "VendorDir is required"
#endif
#ifndef OutputBaseName
  #define OutputBaseName "maestro-setup"
#endif

; Carried over from the WiX UpgradeCode, so the product keeps one identity.
#define AppId "{8236ea1e-6642-439f-ba3c-1f0e65416e31}"
#define AppName "Maestro"
#define AppPublisher "dimms"
#define AppUrl "https://dimms.gr/maestro"
#define AumId "gr.dimms.maestro"

#if Arch == "x86_64"
  #define ArchAllowed "x64compatible"
  #define Arch64Mode "x64compatible"
#elif Arch == "aarch64"
  #define ArchAllowed "arm64"
  #define Arch64Mode "arm64"
#elif Arch == "x86"
  #define ArchAllowed ""
  #define Arch64Mode ""
#else
  #error "Unsupported Arch"
#endif

[Setup]
AppId={{#AppId}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL=https://github.com/dimms0/maestro
AppUpdatesURL=https://github.com/dimms0/maestro/releases
VersionInfoVersion={#Version}
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DisableWelcomePage=no
LicenseFile=..\..\LICENSE.md
SetupIconFile=..\..\assets\logo\maestro.ico
UninstallDisplayIcon={app}\maestro.exe
UninstallDisplayName={#AppName}
; The driver is linked into System32 and entered in Drivers32, so this is a
; machine-wide install and always elevates.
PrivilegesRequired=admin
ArchitecturesAllowed={#ArchAllowed}
ArchitecturesInstallIn64BitMode={#Arch64Mode}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\..
OutputBaseFilename={#OutputBaseName}
CloseApplications=yes
CloseApplicationsFilter=maestro.exe,maestrod.exe

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#BuildDir}\maestro.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\maestrod.exe"; DestDir: "{app}"; Flags: ignoreversion
; Resolved from the exe's own directory by paths::resolve_lib, and linked into
; System32 / SysWOW64 by the [Run] entries below.
Source: "{#BuildDir}\maestrodrv.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\OmniMIDI.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BuildDir}\maestro.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#VendorDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#VendorDir}\*.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\soundfont\*"; DestDir: "{app}\soundfonts"; Flags: ignoreversion

#ifdef Companion1Arch
Source: "{#Companion1BuildDir}\maestrodrv.dll"; DestDir: "{app}\{#Companion1Arch}"; Flags: ignoreversion
Source: "{#Companion1BuildDir}\OmniMIDI.dll"; DestDir: "{app}\{#Companion1Arch}"; Flags: ignoreversion
Source: "{#Companion1BuildDir}\maestro.dll"; DestDir: "{app}\{#Companion1Arch}"; Flags: ignoreversion
Source: "{#Companion1VendorDir}\*.dll"; DestDir: "{app}\{#Companion1Arch}"; Flags: ignoreversion
#endif

#ifdef Companion2Arch
Source: "{#Companion2BuildDir}\maestrodrv.dll"; DestDir: "{app}\{#Companion2Arch}"; Flags: ignoreversion
Source: "{#Companion2BuildDir}\OmniMIDI.dll"; DestDir: "{app}\{#Companion2Arch}"; Flags: ignoreversion
Source: "{#Companion2BuildDir}\maestro.dll"; DestDir: "{app}\{#Companion2Arch}"; Flags: ignoreversion
Source: "{#Companion2VendorDir}\*.dll"; DestDir: "{app}\{#Companion2Arch}"; Flags: ignoreversion
#endif

[Icons]
; The AppUserModelID is not decoration: Windows drops toasts from a process
; with no registered AUMID, and this shortcut is what registers it.
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\maestro.exe"; AppUserModelID: "{#AumId}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\maestro.exe"; AppUserModelID: "{#AumId}"; Tasks: desktopicon

[Run]
; The WinMM driver is of no use until it is linked into System32 and entered in
; Drivers32, so the installer does both instead of leaving them to the System
; Integration tab. Exit codes are ignored: that tab stays the recovery path.
Filename: "{app}\maestro.exe"; Parameters: "install-driver"; StatusMsg: "Installing the MIDI driver..."; Flags: runhidden waituntilterminated
Filename: "{sys}\rundll32.exe"; Parameters: """{app}\maestrodrv.dll"",Maestro_Register"; StatusMsg: "Registering the MIDI driver..."; Flags: runhidden waituntilterminated
; Applications load the Maestro API as maestro.dll from System32 / SysWOW64.
Filename: "{app}\maestro.exe"; Parameters: "install-api"; StatusMsg: "Installing the Maestro API..."; Flags: runhidden waituntilterminated
Filename: "{app}\maestro.exe"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
; Unregister while the files are still on disk.
Filename: "{sys}\rundll32.exe"; Parameters: """{app}\maestrodrv.dll"",Maestro_Unregister"; RunOnceId: "UnregisterDriver"; Flags: runhidden waituntilterminated
Filename: "{app}\maestro.exe"; Parameters: "uninstall-driver"; RunOnceId: "UnlinkDriver"; Flags: runhidden waituntilterminated
Filename: "{app}\maestro.exe"; Parameters: "uninstall-api"; RunOnceId: "UnlinkApi"; Flags: runhidden waituntilterminated

[Code]
const
  UninstallKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall';

{ Both this installer and the WiX package it replaces record themselves in the
  registry view that matches their bitness, which is the install mode's. }
function UninstallRoot: Integer;
begin
  if Is64BitInstallMode then
    Result := HKLM64
  else
    Result := HKLM;
end;

function InstalledIsNewer: Boolean;
var
  Installed: String;
  Have, Want: Int64;
begin
  Result := False;
  if not RegQueryStringValue(UninstallRoot, UninstallKey + '\{#AppId}_is1',
                             'DisplayVersion', Installed) then
    exit;
  if StrToVersion(Installed, Have) and StrToVersion('{#Version}', Want) then
    Result := ComparePackedVersion(Have, Want) > 0;
end;

{ The WiX package this installer replaces. Its files land in the same folder,
  so its uninstaller would later delete ours; retire it before installing. }
function LegacyMsiProductCode(var ProductCode: String): Boolean;
var
  Keys: TArrayOfString;
  Name, Command: String;
  I: Integer;
begin
  Result := False;
  if not RegGetSubkeyNames(UninstallRoot, UninstallKey, Keys) then
    exit;
  for I := 0 to GetArrayLength(Keys) - 1 do
  begin
    if not RegQueryStringValue(UninstallRoot, UninstallKey + '\' + Keys[I],
                               'DisplayName', Name) then
      continue;
    if not SameText(Name, '{#AppName}') then
      continue;
    if not RegQueryStringValue(UninstallRoot, UninstallKey + '\' + Keys[I],
                               'UninstallString', Command) then
      continue;
    if Pos('msiexec', Lowercase(Command)) = 0 then
      continue;
    ProductCode := Keys[I];
    Result := True;
    exit;
  end;
end;

function InitializeSetup(): Boolean;
begin
  Result := not InstalledIsNewer;
  if not Result then
    SuppressibleMsgBox('A newer version of {#AppName} is already installed.',
                       mbError, MB_OK, IDOK);
end;

{ ---- DLLs held by other processes ----

  A DLL that some process has loaded cannot be overwritten or deleted, but it
  can be renamed: the process keeps its mapping, and the next load through the
  System32 / SysWOW64 links picks up the new file at the original path. So
  instead of asking the user to close every app WinMM loaded the driver into,
  the old DLLs are renamed to <name>.old... and deleted once nothing holds
  them, at the latest on the next reboot. }

var
  MovedFrom, MovedTo: TArrayOfString;
  InstallDone: Boolean;

{ The install folder and its immediate subfolders (the companion builds). }
function LibDirs(Root: String): TArrayOfString;
var
  FindRec: TFindRec;
  N: Integer;
begin
  N := 1;
  SetArrayLength(Result, 1);
  Result[0] := Root;
  if FindFirst(Root + '\*', FindRec) then
  try
    repeat
      if (FindRec.Attributes and FILE_ATTRIBUTE_DIRECTORY <> 0) and
         (FindRec.Name <> '.') and (FindRec.Name <> '..') then
      begin
        SetArrayLength(Result, N + 1);
        Result[N] := Root + '\' + FindRec.Name;
        N := N + 1;
      end;
    until not FindNext(FindRec);
  finally
    FindClose(FindRec);
  end;
end;

{ Collected before acting on them, so renames never race the enumeration. }
function FilesIn(Dir, Pattern: String): TArrayOfString;
var
  FindRec: TFindRec;
  N: Integer;
begin
  N := 0;
  SetArrayLength(Result, 0);
  if FindFirst(Dir + '\' + Pattern, FindRec) then
  try
    repeat
      if FindRec.Attributes and FILE_ATTRIBUTE_DIRECTORY = 0 then
      begin
        SetArrayLength(Result, N + 1);
        Result[N] := Dir + '\' + FindRec.Name;
        N := N + 1;
      end;
    until not FindNext(FindRec);
  finally
    FindClose(FindRec);
  end;
end;

procedure DeleteOrSchedule(Path: String);
begin
  if not DeleteFile(Path) then
    RestartReplace(Path, '');
end;

procedure MoveLoadedLibsAside;
var
  Dirs, Files: TArrayOfString;
  D, F, N, Count: Integer;
  Aside: String;
begin
  Count := 0;
  Dirs := LibDirs(ExpandConstant('{app}'));
  for D := 0 to GetArrayLength(Dirs) - 1 do
  begin
    Files := FilesIn(Dirs[D], '*.dll');
    for F := 0 to GetArrayLength(Files) - 1 do
    begin
      N := 1;
      while FileExists(Files[F] + '.old-' + IntToStr(N)) do
        N := N + 1;
      Aside := Files[F] + '.old-' + IntToStr(N);
      if RenameFile(Files[F], Aside) then
      begin
        SetArrayLength(MovedFrom, Count + 1);
        SetArrayLength(MovedTo, Count + 1);
        MovedFrom[Count] := Files[F];
        MovedTo[Count] := Aside;
        Count := Count + 1;
      end
      else
        Log('Could not move ' + Files[F] + ' aside');
    end;
  end;
end;

{ Every .old-* file, from this install or an earlier one whose holder has
  since exited. Ones still loaded go on the next reboot. }
procedure DiscardStaleLibs;
var
  Dirs, Files: TArrayOfString;
  D, F: Integer;
begin
  Dirs := LibDirs(ExpandConstant('{app}'));
  for D := 0 to GetArrayLength(Dirs) - 1 do
  begin
    Files := FilesIn(Dirs[D], '*.old-*');
    for F := 0 to GetArrayLength(Files) - 1 do
      DeleteOrSchedule(Files[F]);
  end;
end;

{ Setup was cancelled or failed after the DLLs were moved: put back the ones
  whose original path is still free, so the existing install keeps working. }
procedure RestoreMovedLibs;
var
  I: Integer;
begin
  for I := 0 to GetArrayLength(MovedFrom) - 1 do
    if not FileExists(MovedFrom[I]) then
      RenameFile(MovedTo[I], MovedFrom[I]);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ProductCode: String;
  ResultCode: Integer;
begin
  Result := '';
  if LegacyMsiProductCode(ProductCode) then
    Exec(ExpandConstant('{sys}\msiexec.exe'),
         '/x ' + ProductCode + ' /qn /norestart', '', SW_HIDE,
         ewWaitUntilTerminated, ResultCode);
  MoveLoadedLibsAside;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    InstallDone := True;
    DiscardStaleLibs;
  end;
end;

procedure DeinitializeSetup;
begin
  if not InstallDone then
    RestoreMovedLibs;
end;

{ The uninstaller leaves behind whatever a running process still holds and
  would otherwise leave the folder too. Those DLLs, any .old-* files and then
  the emptied folders go on the next reboot, without asking for one. Only
  library files are touched, never anything else a user put in the folder. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Dirs, Files: TArrayOfString;
  D, F: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  Dirs := LibDirs(ExpandConstant('{app}'));
  for D := 0 to GetArrayLength(Dirs) - 1 do
  begin
    Files := FilesIn(Dirs[D], '*.dll');
    for F := 0 to GetArrayLength(Files) - 1 do
      DeleteOrSchedule(Files[F]);
    Files := FilesIn(Dirs[D], '*.old-*');
    for F := 0 to GetArrayLength(Files) - 1 do
      DeleteOrSchedule(Files[F]);
  end;
  { Subfolders first: pending deletes run in the order they were scheduled. }
  for D := GetArrayLength(Dirs) - 1 downto 0 do
    if DirExists(Dirs[D]) and not RemoveDir(Dirs[D]) then
      RestartReplace(Dirs[D], '');
end;
