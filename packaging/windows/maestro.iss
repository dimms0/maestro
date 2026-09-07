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
SetupIconFile=..\..\assets\icons\maestro.ico
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
Source: "{#VendorDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#VendorDir}\*.txt"; DestDir: "{app}"; Flags: ignoreversion

#ifdef Companion1Arch
Source: "{#Companion1BuildDir}\maestrodrv.dll"; DestDir: "{app}\{#Companion1Arch}"; Flags: ignoreversion
Source: "{#Companion1BuildDir}\OmniMIDI.dll"; DestDir: "{app}\{#Companion1Arch}"; Flags: ignoreversion
Source: "{#Companion1VendorDir}\*.dll"; DestDir: "{app}\{#Companion1Arch}"; Flags: ignoreversion
#endif

#ifdef Companion2Arch
Source: "{#Companion2BuildDir}\maestrodrv.dll"; DestDir: "{app}\{#Companion2Arch}"; Flags: ignoreversion
Source: "{#Companion2BuildDir}\OmniMIDI.dll"; DestDir: "{app}\{#Companion2Arch}"; Flags: ignoreversion
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
Filename: "{app}\maestro.exe"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
; Unregister while the files are still on disk.
Filename: "{sys}\rundll32.exe"; Parameters: """{app}\maestrodrv.dll"",Maestro_Unregister"; RunOnceId: "UnregisterDriver"; Flags: runhidden waituntilterminated
Filename: "{app}\maestro.exe"; Parameters: "uninstall-driver"; RunOnceId: "UnlinkDriver"; Flags: runhidden waituntilterminated

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
end;
