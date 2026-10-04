; Inno Setup script for the OmniDrop Windows installer.
; Built by .github/workflows/release.yml:
;   ISCC.exe /DAppVersion=1.0.0 /DSourceDir=<bundle dir> /DOutputDir=<dist> packaging\windows\omnidrop.iss

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\build\windows\x64\runner\Release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{8F3C9A51-6B2E-4C7D-9E1A-0D5B7C3E2F41}
AppName=OmniDrop
AppVersion={#AppVersion}
AppVerName=OmniDrop {#AppVersion}
AppPublisher=Eroideches
AppPublisherURL=https://github.com/Eroideches/OmniDrop
AppSupportURL=https://github.com/Eroideches/OmniDrop/issues
DefaultDirName={autopf}\OmniDrop
DefaultGroupName=OmniDrop
DisableProgramGroupPage=yes
OutputDir={#OutputDir}
OutputBaseFilename=omnidrop-windows-x64-setup
SetupIconFile=..\..\windows\runner\resources\app_icon.ico
UninstallDisplayIcon={app}\omnidrop.exe
LicenseFile=..\..\LICENSE
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
PrivilegesRequired=admin
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "firewall"; Description: "Windows Firewall: allow OmniDrop (LAN + Wi-Fi Direct)"; GroupDescription: "Network"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\OmniDrop"; Filename: "{app}\omnidrop.exe"
Name: "{autodesktop}\OmniDrop"; Filename: "{app}\omnidrop.exe"; Tasks: desktopicon

[Run]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\firewall-setup.ps1"" -ExePath ""{app}\omnidrop.exe"""; Flags: runhidden waituntilterminated; Tasks: firewall; StatusMsg: "Configuring Windows Firewall..."
Filename: "{app}\omnidrop.exe"; Description: "{cm:LaunchProgram,OmniDrop}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\firewall-setup.ps1"" -Remove"; Flags: runhidden waituntilterminated; RunOnceId: "OmniDropFirewall"
