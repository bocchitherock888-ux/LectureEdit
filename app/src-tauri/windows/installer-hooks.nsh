; Stops LectureEdit's helper processes that run from this install folder, so an upgrade or
; uninstall never hits locked files. Processes elsewhere (e.g. a user's own llama.cpp) are untouched.
!macro LECTUREEDIT_STOP_HELPERS
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -Command "$$d = '$INSTDIR\'; Get-Process -Name llama-server,lectureedit-loopback -ErrorAction SilentlyContinue | Where-Object { $$_.Path -and $$_.Path.StartsWith($$d, [StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force -ErrorAction SilentlyContinue"`
  Pop $0
  Sleep 300
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro LECTUREEDIT_STOP_HELPERS
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro LECTUREEDIT_STOP_HELPERS
!macroend
