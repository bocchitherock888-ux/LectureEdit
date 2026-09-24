; Stops the app's helper processes that run from this install folder, so an upgrade or
; uninstall never hits locked files. Processes elsewhere (e.g. a user's own llama.cpp) are untouched.
!macro LECTUREEDIT_STOP_HELPERS
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -Command "$$d = '$INSTDIR\'; Get-Process -Name llama-server,lectureedit-loopback -ErrorAction SilentlyContinue | Where-Object { $$_.Path -and $$_.Path.StartsWith($$d, [StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force -ErrorAction SilentlyContinue"`
  Pop $0
  Sleep 300
!macroend

; Up to 0.1.3 the app was installed as "LectureEdit". The installer keys its folder,
; uninstall entry and shortcuts by product name, so the renamed app would otherwise sit
; beside the old one. Run the old uninstaller silently: silent mode never ticks
; "delete application data", and courses, recordings, models and saved keys live under
; the unchanged bundle identifier, so the new install opens them as before.
!macro LECTUREEDIT_REMOVE_LEGACY_INSTALL
  Push $R0
  Push $R1
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\LectureEdit" "InstallLocation"
  StrCpy $R1 $R0 1
  ${If} $R1 == '"'
    StrCpy $R0 $R0 "" 1
    StrCpy $R0 $R0 -1
  ${EndIf}
  ${If} $R0 != ""
  ${AndIf} $R0 != $INSTDIR
  ${AndIf} ${FileExists} "$R0\uninstall.exe"
    DetailPrint "Removing the previous LectureEdit install from $R0"
    ExecWait '"$R0\uninstall.exe" /S _?=$R0'
    Delete "$R0\uninstall.exe"
    RMDir "$R0"
  ${EndIf}
  Pop $R1
  Pop $R0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro LECTUREEDIT_STOP_HELPERS
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro LECTUREEDIT_REMOVE_LEGACY_INSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro LECTUREEDIT_STOP_HELPERS
!macroend
