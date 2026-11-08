; MeowDown 的 NSIS 安装包脚本
;
; 平时不用直接调它，交给 package.ps1：
;   powershell -ExecutionPolicy Bypass -File .\package.ps1 -Version 0.2.0 -Setup
;
; 也可以手动编译（版本号自己换）：
;   makensis /DVERSION=0.2.0 /DVERSION_QUAD=0.2.0.0 ^
;            /DOUTFILE=dist\MeowDown-v0.2.0-setup.exe ^
;            /DSRCDIR=%CD% installer.nsi
;
; 装到当前用户的 %LOCALAPPDATA%\Programs\MeowDown，不写 HKLM、不弹 UAC，
; 因为文件关联本来就只写 HKCU。卸载入口在「设置 → 应用 → 已安装的应用」。

Unicode true
SetCompressor /SOLID lzma
SetCompressorDictSize 64

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

; ---------------------------------------------------------------- 可外部覆盖

!ifndef VERSION
  !define VERSION "0.0.0"
!endif

; VIProductVersion 要四段纯数字，由 package.ps1 算好传进来。
!ifndef VERSION_QUAD
  !define VERSION_QUAD "0.0.0.0"
!endif

!ifndef OUTFILE
  !define OUTFILE "MeowDown-setup.exe"
!endif

; 源文件所在的项目根目录。用绝对路径，免得依赖编译时的工作目录。
!ifndef SRCDIR
  !define SRCDIR "."
!endif

!define APP_NAME    "MeowDown"
!define APP_EXE     "MeowDown.exe"
!define APP_KEY     "Software\MeowDown"
!define UNINST_KEY  "Software\Microsoft\Windows\CurrentVersion\Uninstall\MeowDown"
!define APP_URL     "https://github.com/xarzhi/MeowDown"

; ---------------------------------------------------------------- 安装程序信息

Name "${APP_NAME} ${VERSION}"
Caption "${APP_NAME} ${VERSION} 安装"
BrandingText "${APP_NAME} ${VERSION}"
OutFile "${OUTFILE}"

InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
; 升级安装时沿用上次选的目录。
InstallDirRegKey HKCU "${APP_KEY}" "InstallDir"

; 只写 HKCU，所以不要管理员权限；这样双击就能装完。
RequestExecutionLevel user

ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${VERSION_QUAD}"
VIAddVersionKey "ProductName"     "${APP_NAME}"
VIAddVersionKey "ProductVersion"  "${VERSION}"
VIAddVersionKey "FileVersion"     "${VERSION}"
VIAddVersionKey "FileDescription" "${APP_NAME} 安装程序"
VIAddVersionKey "CompanyName"     "xarzhi"
VIAddVersionKey "LegalCopyright"  "字体部分见 OFL.txt"

; ---------------------------------------------------------------- 界面

!define MUI_ICON   "${SRCDIR}\assets\app.ico"
!define MUI_UNICON "${SRCDIR}\assets\app.ico"
!define MUI_ABORTWARNING

!define MUI_WELCOMEPAGE_TITLE "安装 ${APP_NAME} ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "MeowDown 是一个所见即所得的 Markdown 编辑器。$\r$\n$\r$\n程序会安装到当前用户目录，不需要管理员权限；文件关联也只写当前用户。$\r$\n$\r$\n点「下一步」继续。"

!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "立即运行 ${APP_NAME}"
!define MUI_FINISHPAGE_LINK "项目主页"
!define MUI_FINISHPAGE_LINK_LOCATION "${APP_URL}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

; 宋体在安装界面上太老气，能换就换。
SetFont /LANG=${LANG_SIMPCHINESE} "Microsoft YaHei UI" 9
SetFont /LANG=${LANG_ENGLISH} "Segoe UI" 9

; ---------------------------------------------------------------- 文案

LangString SEC_MAIN      ${LANG_SIMPCHINESE} "MeowDown 程序（必需）"
LangString SEC_MAIN      ${LANG_ENGLISH}     "MeowDown program (required)"
LangString SEC_STARTMENU ${LANG_SIMPCHINESE} "开始菜单快捷方式"
LangString SEC_STARTMENU ${LANG_ENGLISH}     "Start menu shortcut"
LangString SEC_DESKTOP   ${LANG_SIMPCHINESE} "桌面快捷方式"
LangString SEC_DESKTOP   ${LANG_ENGLISH}     "Desktop shortcut"
LangString SEC_ASSOC     ${LANG_SIMPCHINESE} "把 .md 交给 MeowDown 打开"
LangString SEC_ASSOC     ${LANG_ENGLISH}     "Open .md files with MeowDown"

LangString DESC_SEC_MAIN      ${LANG_SIMPCHINESE} "主程序、说明文档和字体许可证。"
LangString DESC_SEC_MAIN      ${LANG_ENGLISH}     "The program, readme and font licence."
LangString DESC_SEC_STARTMENU ${LANG_SIMPCHINESE} "在开始菜单里放一个 MeowDown 和一个卸载快捷方式。"
LangString DESC_SEC_STARTMENU ${LANG_ENGLISH}     "Add MeowDown and its uninstaller to the Start menu."
LangString DESC_SEC_DESKTOP   ${LANG_SIMPCHINESE} "在桌面上放一个 MeowDown 快捷方式。"
LangString DESC_SEC_DESKTOP   ${LANG_ENGLISH}     "Put a MeowDown shortcut on the desktop."
LangString DESC_SEC_ASSOC     ${LANG_SIMPCHINESE} "把 .md / .markdown / .mdown 关联到 MeowDown（只改当前用户）。Windows 10/11 上系统仍会让你确认一次：设置 → 默认应用 → 按文件类型指定默认应用。"
LangString DESC_SEC_ASSOC     ${LANG_ENGLISH}     "Associate .md / .markdown / .mdown with MeowDown for the current user. Windows 10/11 still asks once in Settings > Default apps."

; ---------------------------------------------------------------- 安装

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "MeowDown 只支持 64 位 Windows。"
    Abort
  ${EndIf}
  ; 32 位的安装程序默认会写进 Wow6432Node，关联就白写了。
  SetRegView 64
  ; 快捷方式只放进当前用户，和下面的 HKCU 写入保持一致。
  SetShellVarContext current
FunctionEnd

Section "$(SEC_MAIN)" SecMain
  SectionIn RO

  ; 覆盖正在运行的 exe 会失败，早点拦下来比装到一半报错好。
  ; 以追加方式打开只是探一下独占锁，不写任何东西。
  ${If} ${FileExists} "$INSTDIR\${APP_EXE}"
    ClearErrors
    FileOpen $9 "$INSTDIR\${APP_EXE}" a
    ${If} ${Errors}
      MessageBox MB_ICONSTOP|MB_OK "MeowDown 正在运行，请先退出它再安装。"
      Quit
    ${EndIf}
    FileClose $9
  ${EndIf}

  SetOutPath "$INSTDIR"
  SetOverwrite on

  File "/oname=${APP_EXE}" "${SRCDIR}\target\release\meowdown.exe"
  File "${SRCDIR}\README.md"
  File "${SRCDIR}\assets\fonts\OFL.txt"

  WriteUninstaller "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "${APP_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${APP_KEY}" "Version" "${VERSION}"

  ; 让它在「设置 → 应用 → 已安装的应用」里出现，并且能正常卸载。
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APP_NAME}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr   HKCU "${UNINST_KEY}" "Publisher"       "xarzhi"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${APP_EXE},0"
  WriteRegStr   HKCU "${UNINST_KEY}" "URLInfoAbout"    "${APP_URL}"
  WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr   HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "$(SEC_STARTMENU)" SecStartMenu
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "$(SEC_DESKTOP)" SecDesktop
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
SectionEnd

Section "$(SEC_ASSOC)" SecAssoc
  ; 复用程序自己的注册逻辑，免得注册表写法在两处各维护一份。
  ExecWait '"$INSTDIR\${APP_EXE}" --register'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain}      $(DESC_SEC_MAIN)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecStartMenu} $(DESC_SEC_STARTMENU)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop}   $(DESC_SEC_DESKTOP)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAssoc}     $(DESC_SEC_ASSOC)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function .onInstSuccess
  ; 安装程序已经问过一遍了。不勾关联时也记下这一点，免得装完第一次
  ; 启动程序又自作主张把 .md 抢过去；之后仍可在设置里手动开。
  WriteRegDWORD HKCU "${APP_KEY}" "AssociationAsked" 1
FunctionEnd

; ---------------------------------------------------------------- 卸载

Function un.onInit
  SetRegView 64
  SetShellVarContext current
FunctionEnd

Section "Uninstall"
  ; 先让程序自己把关联撤掉，此时 exe 还在。
  ${If} ${FileExists} "$INSTDIR\${APP_EXE}"
    ExecWait '"$INSTDIR\${APP_EXE}" --unregister'
  ${EndIf}

  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  ; 只删自己认识的文件，不 RMDir /r —— 用户可能把安装目录选到了别处。
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\OFL.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINST_KEY}"
  ; 这个键下只有安装信息（InstallDir/Version/Capabilities/AssociationAsked），
  ; 真正的内容设置存在 %APPDATA%\MeowDown\settings.json，这里不动它。
  DeleteRegKey HKCU "${APP_KEY}"
SectionEnd
