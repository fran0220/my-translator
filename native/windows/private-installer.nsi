Unicode true
!include "MUI2.nsh"
!ifndef APP_EXE
!error "Pass /DAPP_EXE=absolute-path-to-private-translator-native.exe"
!endif
!ifndef OUTPUT_FILE
!error "Pass /DOUTPUT_FILE=absolute-installer-path"
!endif
Name "实时翻译 Native 内部版"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\TranslatorNativePrivate"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Section "原生内部版"
  ; Never overwrite an existing installation or another application's files.
  IfFileExists "$INSTDIR\*.*" 0 fresh
    MessageBox MB_ICONSTOP "目标目录已存在文件，请选择新的空目录。"
    Abort
  fresh:
  SetOutPath "$INSTDIR"
  File /oname=translator-native.exe "${APP_EXE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\实时翻译 Native 内部版.lnk" "$INSTDIR\translator-native.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\TranslatorNativePrivate" "DisplayName" "实时翻译 Native 内部版"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\TranslatorNativePrivate" "DisplayVersion" "0.9.1"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\TranslatorNativePrivate" "UninstallString" '"$INSTDIR\uninstall.exe"'
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\translator-native.exe"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$SMPROGRAMS\实时翻译 Native 内部版.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\TranslatorNativePrivate"
  RMDir "$INSTDIR"
  ; Deliberately preserve all user settings, including native settings-team.json.
SectionEnd
