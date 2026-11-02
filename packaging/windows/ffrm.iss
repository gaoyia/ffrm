; 当前用户安装包。配置文件写在程序旁边，所以不装进需要管理员权限的目录。

#ifndef MyAppVersion
  #define MyAppVersion "0.1.6"
#endif

#define MyAppName "ffrm"
#define MyAppExeName "ffrm.exe"

[Setup]
AppId={{788F2332-28C6-4107-9D5E-75DBABDF8EDD}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=gaoyia
AppPublisherURL=https://github.com/gaoyia/ffrm
AppSupportURL=https://github.com/gaoyia/ffrm
DefaultDirName={localappdata}\ffrm
DefaultGroupName=文件强制解锁删除工具
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\..\target\release
OutputBaseFilename=ffrm-setup
SetupIconFile=..\..\assets\ffrm.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "..\..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\target\release\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\文件强制解锁删除工具"; Filename: "{app}\{#MyAppExeName}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "启动文件强制解锁删除工具"; Flags: nowait postinstall skipifsilent
