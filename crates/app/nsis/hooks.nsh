!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$DESKTOP\Blah Blah Bot.lnk" "$INSTDIR\Blah Blah Bot.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$DESKTOP\Blah Blah Bot.lnk"
!macroend
