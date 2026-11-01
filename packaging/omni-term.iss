; Instalador de Windows (Inno Setup). Lo compila el workflow de release:
;   ISCC /DAppVersion=0.1.6 /DSourceDir=<carpeta con omni-term.exe> /DOutputDir=dist /DOutputName=<nombre> packaging\omni-term.iss
; Instalación por usuario (sin administrador), con acceso directo, desinstalador y opción de añadir al PATH.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#ifndef OutputName
  #define OutputName "omni-term-setup"
#endif

[Setup]
AppId={{A872BB96-60D0-46D5-A3DD-68A24FB47501}
AppName=omni-term
AppVersion={#AppVersion}
AppPublisher=pdf3rn
AppPublisherURL=https://github.com/pdf3rn/omni-term
AppSupportURL=https://github.com/pdf3rn/omni-term/issues
DefaultDirName={autopf}\omni-term
DefaultGroupName=omni-term
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=omni-term.ico
UninstallDisplayIcon={app}\omni-term.exe
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
LicenseFile=..\LICENSE-MIT

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Añadir omni-term al PATH"; GroupDescription: "Opciones:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\omni-term.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\omni-term"; Filename: "{app}\omni-term.exe"
Name: "{autodesktop}\omni-term"; Filename: "{app}\omni-term.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\omni-term.exe"; Description: "{cm:LaunchProgram,omni-term}"; Flags: nowait postinstall skipifsilent

[Registry]
; PATH del usuario (o del sistema si se instala para todos).
Root: HKA; Subkey: "{code:EnvironmentKey}"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Code]
function EnvironmentKey(Param: string): string;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function NeedsAddPath(Param: string): Boolean;
var
  Existing: string;
  Root: Integer;
begin
  if IsAdminInstallMode then Root := HKEY_LOCAL_MACHINE else Root := HKEY_CURRENT_USER;
  if not RegQueryStringValue(Root, EnvironmentKey(''), 'Path', Existing) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(Existing) + ';') = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Existing, Target: string;
  Root, P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then exit;
  if IsAdminInstallMode then Root := HKEY_LOCAL_MACHINE else Root := HKEY_CURRENT_USER;
  if not RegQueryStringValue(Root, EnvironmentKey(''), 'Path', Existing) then exit;
  Target := ';' + ExpandConstant('{app}');
  P := Pos(Uppercase(Target), Uppercase(Existing));
  if P > 0 then
  begin
    Delete(Existing, P, Length(Target));
    RegWriteExpandStringValue(Root, EnvironmentKey(''), 'Path', Existing);
  end;
end;
