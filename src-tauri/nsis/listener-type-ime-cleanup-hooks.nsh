!macro LISTENER_TYPE_LEGACY_IME_UNREGISTER_X64
  ${If} ${RunningX64}
    IfFileExists "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll" 0 +4
      DetailPrint "Unregistering legacy Listener Type x64 TSF IME"
      ExecWait '"$WINDIR\Sysnative\regsvr32.exe" /s /u "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll"' $0
      DetailPrint "Legacy Listener Type x64 TSF IME unregister exit code $0"
  ${EndIf}
!macroend

!macro LISTENER_TYPE_LEGACY_IME_UNREGISTER_X86
  IfFileExists "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll" 0 +7
    DetailPrint "Unregistering legacy Listener Type x86 TSF IME"
    ${If} ${RunningX64}
      ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s /u "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"' $0
    ${Else}
      ExecWait '"$WINDIR\System32\regsvr32.exe" /s /u "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"' $0
    ${EndIf}
    DetailPrint "Legacy Listener Type x86 TSF IME unregister exit code $0"
!macroend

!macro LISTENER_TYPE_LEGACY_IME_REMOVE_FILES
  Delete "$INSTDIR\windows-ime\x64\ListenerTypeIme.dll"
  Delete "$INSTDIR\windows-ime\x86\ListenerTypeIme.dll"
  RMDir "$INSTDIR\windows-ime\x64"
  RMDir "$INSTDIR\windows-ime\x86"
  RMDir "$INSTDIR\windows-ime"
!macroend

!macro LISTENER_TYPE_LEGACY_IME_REMOVE_REGISTRY
  ${If} ${RunningX64}
    SetRegView 64
  ${EndIf}
  DeleteRegKey HKLM "Software\Microsoft\CTF\TIP\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}"
  DeleteRegKey HKLM "Software\Classes\CLSID\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}"
  ${If} ${RunningX64}
    SetRegView 32
    DeleteRegKey HKLM "Software\Classes\CLSID\{E6D16C6C-2975-4A5C-BBBB-67A3C9966767}"
    SetRegView lastused
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro LISTENER_TYPE_LEGACY_IME_UNREGISTER_X86
  !insertmacro LISTENER_TYPE_LEGACY_IME_UNREGISTER_X64
  !insertmacro LISTENER_TYPE_LEGACY_IME_REMOVE_REGISTRY
  !insertmacro LISTENER_TYPE_LEGACY_IME_REMOVE_FILES
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro LISTENER_TYPE_LEGACY_IME_UNREGISTER_X86
  !insertmacro LISTENER_TYPE_LEGACY_IME_UNREGISTER_X64
  !insertmacro LISTENER_TYPE_LEGACY_IME_REMOVE_REGISTRY
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro LISTENER_TYPE_LEGACY_IME_REMOVE_FILES
!macroend
