!macro LISTENER_TYPE_IME_ABORT_IF_FAILED EXIT_CODE LABEL
  ${If} ${EXIT_CODE} != 0
    DetailPrint "Listener Type TSF IME ${LABEL} failed with exit code ${EXIT_CODE}"
    Abort
  ${EndIf}
!macroend

!macro LISTENER_TYPE_IME_REGISTER_X64
  ${If} ${RunningX64}
    DetailPrint "Registering Listener Type x64 TSF IME"
    ExecWait '"$WINDIR\Sysnative\regsvr32.exe" /s "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll"' $0
    ${If} $0 != 0
      ${DisableX64FSRedirection}
      ExecWait '"$WINDIR\System32\regsvr32.exe" /s "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll"' $0
      ${EnableX64FSRedirection}
    ${EndIf}
    !insertmacro LISTENER_TYPE_IME_ABORT_IF_FAILED $0 "x64 registration"
  ${EndIf}
!macroend

!macro LISTENER_TYPE_IME_UNREGISTER_X64
  ${If} ${RunningX64}
    DetailPrint "Unregistering Listener Type x64 TSF IME"
    ExecWait '"$WINDIR\Sysnative\regsvr32.exe" /s /u "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll"' $0
    ${If} $0 != 0
      ${DisableX64FSRedirection}
      ExecWait '"$WINDIR\System32\regsvr32.exe" /s /u "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll"' $0
      ${EnableX64FSRedirection}
    ${EndIf}
    DetailPrint "Listener Type x64 TSF IME unregister exit code $0"
  ${EndIf}
!macroend

!macro LISTENER_TYPE_IME_UNREGISTER_X86
  DetailPrint "Unregistering Listener Type x86 TSF IME"
  ${If} ${RunningX64}
    ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s /u "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"' $0
  ${Else}
    ExecWait '"$WINDIR\System32\regsvr32.exe" /s /u "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"' $0
  ${EndIf}
  DetailPrint "Listener Type x86 TSF IME unregister exit code $0"
!macroend

!macro LISTENER_TYPE_IME_REGISTER_X86
  DetailPrint "Registering Listener Type x86 TSF IME"
  ${If} ${RunningX64}
    ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"' $0
  ${Else}
    ExecWait '"$WINDIR\System32\regsvr32.exe" /s "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"' $0
  ${EndIf}
  ${If} $0 != 0
    StrCpy $1 $0
    !insertmacro LISTENER_TYPE_IME_UNREGISTER_X64
    StrCpy $0 $1
  ${EndIf}
  !insertmacro LISTENER_TYPE_IME_ABORT_IF_FAILED $0 "x86 registration"
!macroend

!macro LISTENER_TYPE_IME_STAGE_AND_REPLACE PLATFORM_DIR ENV_VAR
  ; Write new bytes to ListenerTypeIme.dll.new, then atomically replace the old file.
  ; If the old DLL is still mapped into TSF client processes (browsers, IDEs,
  ; chat apps, ...) and Delete fails, queue the rename for next reboot via
  ; PendingFileRenameOperations. Issue #321.
  SetOutPath "$INSTDIR\windows-ime\${PLATFORM_DIR}"
  File /oname=ListenerTypeIme.dll.new "$%${ENV_VAR}%"
  ClearErrors
  Delete "$INSTDIR\windows-ime\${PLATFORM_DIR}\ListenerTypeIme.dll"
  ${If} ${Errors}
    DetailPrint "ListenerTypeIme.dll (${PLATFORM_DIR}) is in use; queuing replacement at next reboot"
    Rename /REBOOTOK "$INSTDIR\windows-ime\${PLATFORM_DIR}\ListenerTypeIme.dll.new" "$INSTDIR\windows-ime\${PLATFORM_DIR}\ListenerTypeIme.dll"
    SetRebootFlag true
  ${Else}
    Rename "$INSTDIR\windows-ime\${PLATFORM_DIR}\ListenerTypeIme.dll.new" "$INSTDIR\windows-ime\${PLATFORM_DIR}\ListenerTypeIme.dll"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Upgrade path: release the COM/TSF registration so new client processes won't
  ; bind to the old DLL while we replace it. Fresh install: regsvr32 /u against
  ; a missing DLL is harmless — the existing UNREGISTER macros log the exit code
  ; and continue without aborting.
  !insertmacro LISTENER_TYPE_IME_UNREGISTER_X86
  !insertmacro LISTENER_TYPE_IME_UNREGISTER_X64

  !insertmacro LISTENER_TYPE_IME_STAGE_AND_REPLACE "x64" "LISTENER_TYPE_IME_DLL_X64"
  !insertmacro LISTENER_TYPE_IME_STAGE_AND_REPLACE "x86" "LISTENER_TYPE_IME_DLL_X86"

  SetOutPath "$INSTDIR"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro LISTENER_TYPE_IME_REGISTER_X64
  !insertmacro LISTENER_TYPE_IME_REGISTER_X86
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro LISTENER_TYPE_IME_UNREGISTER_X86
  !insertmacro LISTENER_TYPE_IME_UNREGISTER_X64
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll"
  Delete "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"
  RMDir "$INSTDIR\windows-ime\x64"
  RMDir "$INSTDIR\windows-ime\x86"
  RMDir "$INSTDIR\windows-ime"
!macroend
