; A fresh helper is embedded in both installer and uninstaller. An older installed CLI need not know its command.
!define VATRA_SERVICE_HELPER "${__FILEDIR__}\binaries\vatra-x86_64-pc-windows-msvc.exe"

!macro VatraStopService
  InitPluginsDir
  File /oname=$PLUGINSDIR\vatra-service-control.exe "${VATRA_SERVICE_HELPER}"
  nsExec::ExecToStack '"$PLUGINSDIR\vatra-service-control.exe" service-shutdown-owned --data-dir "$APPDATA\Vatra" --expected-exe "$INSTDIR\vatra.exe"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_ICONSTOP|MB_OK "Vatra could not safely stop its service. Close this copy of Vatra and retry. / Vatra nu a putut opri serviciul în siguranță. Închide această copie și încearcă din nou." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
!macroend

; Replace Tauri's Restart Manager force-shutdown fallback. Busy executables cause an explicit refusal.
!macroundef CheckIfAppIsRunning
!macro CheckIfAppIsRunning executablePath productName
  nsExec::ExecToStack '"$PLUGINSDIR\vatra-service-control.exe" service-check-unlocked --exe "${executablePath}"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_ICONSTOP|MB_OK "Close ${productName} before continuing. / Închide ${productName} înainte de a continua." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
!macroend
!macro NSIS_HOOK_PREINSTALL
  !insertmacro VatraStopService
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro VatraStopService
!macroend
