!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$DESKTOP\Blah Blah Bot.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$DESKTOP\Blah Blah Bot.lnk"
!macroend
