VIAddVersionKey "CompanyName" "Radoslaw Wojcik"
VIAddVersionKey "InternalName" "Blah-Blah-Bot-setup"
VIAddVersionKey "OriginalFilename" "Blah-Blah-Bot-setup.exe"
VIAddVersionKey "Comments" "Local meeting recorder installer (https://github.com/Radek-Wojcik/blah-blah-bot)"

Function VerifyWritableInstDir
  StrCpy $0 0
  IfFileExists "$INSTDIR\*.*" 0 +2
    StrCpy $0 1

  ${If} $0 = 0
    CreateDirectory "$INSTDIR"
  ${EndIf}

  ClearErrors
  FileOpen $1 "$INSTDIR\.bbb-write-test.tmp" w
  IfErrors not_writable
  FileClose $1
  Delete "$INSTDIR\.bbb-write-test.tmp"
  Goto writable_done

  not_writable:
    ${If} $0 = 0
      RMDir "$INSTDIR"
    ${EndIf}
    MessageBox MB_OK|MB_ICONEXCLAMATION /SD IDOK "The selected folder cannot be written to:$\r$\n$INSTDIR$\r$\n$\r$\nPlease choose a folder you have permission to write to, such as $LOCALAPPDATA\${PRODUCTNAME}, rather than Program Files."
    Abort

  writable_done:
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  Call VerifyWritableInstDir
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$DESKTOP\Blah Blah Bot.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Keep the shortcut and registry data when this uninstall is part of an update.
  ${If} $UpdateMode <> 1
    Delete "$DESKTOP\Blah Blah Bot.lnk"
    DeleteRegValue SHCTX "${MANUPRODUCTKEY}" ""
    DeleteRegKey /ifempty SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
  ${EndIf}
!macroend
