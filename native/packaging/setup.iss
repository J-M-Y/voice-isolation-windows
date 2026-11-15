#ifndef SourceDir
  #error SourceDir must point to the staged release folder
#endif
#ifndef OutputDir
  #error OutputDir must point to the release assets folder
#endif

[Setup]
AppId={{BD0956BC-30E2-4D4F-A3A7-E0F034C0C1A0}
AppName=Voice Isolation
AppVersion=2.0.0
AppPublisher=J-M-Y
AppPublisherURL=https://github.com/J-M-Y/voice-isolation-windows
DefaultDirName={localappdata}\Programs\VoiceIsolation
DefaultGroupName=Voice Isolation
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=VoiceIsolation-v2.0.0-win-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\VoiceIsolation.exe

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Voice Isolation"; Filename: "{app}\VoiceIsolation.exe"
Name: "{autodesktop}\Voice Isolation"; Filename: "{app}\VoiceIsolation.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\VoiceIsolation.exe"; Description: "{cm:LaunchProgram,Voice Isolation}"; Flags: nowait postinstall skipifsilent
