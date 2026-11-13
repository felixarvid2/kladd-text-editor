; Inno Setup-skript för Kladd. Bygg från repots rot:
;   iscc /DAppVersion=1.7.0 packaging\windows\kladd.iss
; Kräver att `cargo build --release` redan har skapat target\release\kladd.exe.
; Installationsprogrammet hamnar i dist\.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "Kladd"
#define AppExe "kladd.exe"
#define AppPublisher "Felix"
#define AppURL "https://github.com/felixarvid2/kladd-text-editor"
; Sökvägar nedan är relativa till den här filen, inte till arbetskatalogen.
#define Root "..\.."

[Setup]
; Fast GUID så att uppdateringar ersätter den gamla installationen. Ändra aldrig.
AppId={{6F1C2A7E-5B3D-4E8A-9C41-2D7B0E5A9F13}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
VersionInfoVersion={#AppVersion}
VersionInfoDescription={#AppName}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
SetupIconFile={#Root}\packaging\icons\kladd.ico
LicenseFile={#Root}\LICENSE
OutputDir={#Root}\dist
OutputBaseFilename=Kladd-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Per användare som standard (ingen administratör); dialogen låter den som vill installera för alla.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes

[Languages]
Name: "swedish"; MessagesFile: "Swedish.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
swedish.AssocTask=Visa Kladd under "Öppna med" för filtyperna:
english.AssocTask=Show Kladd under "Open with" for file types:
swedish.AssocDocx=Word-dokument (.docx)
english.AssocDocx=Word documents (.docx)
swedish.AssocOdt=OpenDocument-text (.odt)
english.AssocOdt=OpenDocument text (.odt)
swedish.AssocMd=Markdown (.md)
english.AssocMd=Markdown (.md)
swedish.AssocTxt=Textfiler (.txt)
english.AssocTxt=Text files (.txt)
swedish.ProgIdName=Kladd-dokument
english.ProgIdName=Kladd document

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "assoc"; Description: "{cm:AssocTask}"; Flags: unchecked
Name: "assoc\docx"; Description: "{cm:AssocDocx}"; Flags: unchecked
Name: "assoc\odt"; Description: "{cm:AssocOdt}"; Flags: unchecked
Name: "assoc\md"; Description: "{cm:AssocMd}"; Flags: unchecked
Name: "assoc\txt"; Description: "{cm:AssocTxt}"; Flags: unchecked

[Files]
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\assets\fonts\OFL.txt"; DestDir: "{app}\licenses"; DestName: "FONT-LICENSE.txt"; Flags: ignoreversion
Source: "{#Root}\assets\fonts\Carlito-OFL.txt"; DestDir: "{app}\licenses"; DestName: "CARLITO-FONT-LICENSE.txt"; Flags: ignoreversion
Source: "{#Root}\assets\fonts\STIXTwo-OFL.txt"; DestDir: "{app}\licenses"; DestName: "STIXTWO-FONT-LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Programmet självt, så att Windows kan visa namn och ikon under "Öppna med".
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc
; Egen ProgID med ikon och öppna-kommando.
Root: HKA; Subkey: "Software\Classes\Kladd.Document"; ValueType: string; ValueData: "{cm:ProgIdName}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Kladd.Document\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Kladd.Document\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc
; Lägg bara till Kladd bland "Öppna med"-förslagen; standardprogrammet rörs inte.
Root: HKA; Subkey: "Software\Classes\.docx\OpenWithProgids"; ValueType: string; ValueName: "Kladd.Document"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc\docx
Root: HKA; Subkey: "Software\Classes\.odt\OpenWithProgids"; ValueType: string; ValueName: "Kladd.Document"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc\odt
Root: HKA; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "Kladd.Document"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc\md
Root: HKA; Subkey: "Software\Classes\.txt\OpenWithProgids"; ValueType: string; ValueName: "Kladd.Document"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc\txt

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
