; Author: Ray Zhang
Unicode true
!include "MUI2.nsh"
!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!ifndef PAYLOAD_ROOT
  !error "PAYLOAD_ROOT is required"
!endif
!ifndef OUTPUT_EXE
  !error "OUTPUT_EXE is required"
!endif
!ifndef UNINSTALL_FILES
  !error "UNINSTALL_FILES is required"
!endif

Name "RY SSH 远程支持"
OutFile "${OUTPUT_EXE}"
InstallDir "$LOCALAPPDATA\Programs\RY SSH Remote Support"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show
VIProductVersion "1.1.1.0"
VIAddVersionKey /LANG=2052 "ProductName" "RY SSH 远程支持"
VIAddVersionKey /LANG=2052 "FileDescription" "主控端与被控端安装程序"
VIAddVersionKey /LANG=2052 "CompanyName" "Ray Zhang"
VIAddVersionKey /LANG=2052 "LegalCopyright" "Ray Zhang"
VIAddVersionKey /LANG=2052 "FileVersion" "1.1.1"

Var Role
Var Dialog
Var ControllerRadio
Var ControlledRadio
Var Selected
Var Parameters

!define MUI_ABORTWARNING
Page custom RolePage RoleLeave
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TITLE "安装完成"
!define MUI_FINISHPAGE_TEXT "点击“完成”启动所选角色。被控端启动时需要管理员权限，并会提示填写现场连接信息。"
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "启动所选角色"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchRole
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  ${GetParameters} $Parameters
  ClearErrors
  ${GetOptions} $Parameters "/ROLE=" $Role
  ${If} $Role != "controller"
  ${AndIf} $Role != "controlled"
    StrCpy $Role ""
  ${EndIf}
  IfSilent 0 ready
  ${If} $Role == ""
    SetErrorLevel 2
    Abort
  ${EndIf}
  ready:
FunctionEnd

Function RolePage
  !insertmacro MUI_HEADER_TEXT "这台电脑用来做什么？" "选择主控端或被控端，然后点击“安装”。"
  nsDialogs::Create 1018
  Pop $Dialog
  ${If} $Dialog == error
    Abort
  ${EndIf}
  ${NSD_CreateRadioButton} 0 5u 100% 18u "主控端 — 我来连接、管理其他电脑"
  Pop $ControllerRadio
  ${NSD_CreateLabel} 14u 26u 95% 28u "输入现场提供的配对码，可连接多台被控电脑，也可结束指定连接。"
  Pop $0
  ${NSD_CreateRadioButton} 0 67u 100% 18u "被控端 — 允许支持人员连接这台电脑"
  Pop $ControlledRadio
  ${NSD_CreateLabel} 14u 88u 95% 29u "启动后配置本机 SSH 和后端连接，显示一次性配对码。保持窗口运行即可。"
  Pop $0
  ${NSD_CreateLabel} 0 133u 100% 31u "安装包可在两边重复使用。程序不会自动开机连接；每次远程支持都需要启动并配对。"
  Pop $0
  ${If} $Role == "controlled"
    ${NSD_Check} $ControlledRadio
  ${Else}
    ${NSD_Check} $ControllerRadio
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function RoleLeave
  ${NSD_GetState} $ControlledRadio $Selected
  ${If} $Selected == ${BST_CHECKED}
    StrCpy $Role "controlled"
  ${Else}
    StrCpy $Role "controller"
  ${EndIf}
FunctionEnd

Section "安装"
  IfFileExists "$INSTDIR\ry-ssh-tunnel.exe" 0 install_files
  ClearErrors
  FileOpen $0 "$INSTDIR\ry-ssh-tunnel.exe" a
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONEXCLAMATION "请先结束正在运行的 RY SSH 会话，再重新安装。" /SD IDOK
    SetErrorLevel 3
    Quit
  ${EndIf}
  FileClose $0
  install_files:
  ; All payload files are fixed by the build manifest. No runtime/private keys ship.
  SetOutPath "$INSTDIR"
  File /r "${PAYLOAD_ROOT}\*.*"
  FileOpen $0 "$INSTDIR\role.txt" w
  FileWrite $0 "$Role"
  FileClose $0
  WriteUninstaller "$INSTDIR\卸载.exe"
  CreateDirectory "$SMPROGRAMS\RY SSH 远程支持"
  CreateShortcut "$SMPROGRAMS\RY SSH 远程支持\主控端.lnk" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\scripts\windows\Start-Controller.ps1"' "$INSTDIR\ry-ssh-tunnel.exe"
  CreateShortcut "$SMPROGRAMS\RY SSH 远程支持\被控端.lnk" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\scripts\windows\Start-Controlled.ps1"' "$INSTDIR\ry-ssh-tunnel.exe"
  CreateShortcut "$SMPROGRAMS\RY SSH 远程支持\仅安装后端SSH.lnk" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\scripts\windows\Start-Controlled.ps1" -BackendOnly' "$INSTDIR\ry-ssh-tunnel.exe"
  CreateShortcut "$SMPROGRAMS\RY SSH 远程支持\卸载.lnk" "$INSTDIR\卸载.exe"
  ${If} $Role == "controlled"
    CreateShortcut "$DESKTOP\RY SSH 远程支持.lnk" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\scripts\windows\Start-Controlled.ps1"' "$INSTDIR\ry-ssh-tunnel.exe"
  ${Else}
    CreateShortcut "$DESKTOP\RY SSH 远程支持.lnk" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\scripts\windows\Start-Controller.ps1"' "$INSTDIR\ry-ssh-tunnel.exe"
  ${EndIf}
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel" "DisplayName" "RY SSH 远程支持"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel" "DisplayVersion" "1.1.1"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel" "Publisher" "Ray Zhang"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel" "UninstallString" '"$INSTDIR\卸载.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel" "NoRepair" 1
SectionEnd

Function LaunchRole
  ${If} $Role == "controlled"
    ExecShell "open" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\scripts\windows\Start-Controlled.ps1"'
  ${Else}
    ExecShell "open" "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\scripts\windows\Start-Controller.ps1"'
  ${EndIf}
FunctionEnd

Section "Uninstall"
  IfFileExists "$INSTDIR\ry-ssh-tunnel.exe" 0 uninstall_files
  ClearErrors
  FileOpen $0 "$INSTDIR\ry-ssh-tunnel.exe" a
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONEXCLAMATION "请先结束正在运行的 RY SSH 会话，再卸载程序。" /SD IDOK
    SetErrorLevel 3
    Quit
  ${EndIf}
  FileClose $0
  uninstall_files:
  ; Never remove OpenSSH services, user sessions, unrelated files or machine keys.
  !include "${UNINSTALL_FILES}"
  Delete "$INSTDIR\role.txt"
  Delete "$INSTDIR\卸载.exe"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\RY SSH 远程支持.lnk"
  Delete "$SMPROGRAMS\RY SSH 远程支持\主控端.lnk"
  Delete "$SMPROGRAMS\RY SSH 远程支持\被控端.lnk"
  Delete "$SMPROGRAMS\RY SSH 远程支持\仅安装后端SSH.lnk"
  Delete "$SMPROGRAMS\RY SSH 远程支持\卸载.lnk"
  RMDir "$SMPROGRAMS\RY SSH 远程支持"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\RYSSHTunnel"
SectionEnd
