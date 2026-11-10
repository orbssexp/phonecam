; Register / unregister the softcam DirectShow filter ("PhoneCam" camera). perMachine install
; runs elevated, which regsvr32 for HKLM needs. Resources land next to the exe ($INSTDIR).
;
; Any app that has listed cameras (browsers, Zoom, ...) keeps softcam.dll loaded, and Windows
; refuses to overwrite a loaded DLL but does allow renaming it. So before files are copied the
; old DLL is unregistered and moved aside to a temp name (deleted at next reboot); the installer
; then writes a fresh file and nobody has to close anything.
!macro PHONECAM_RETIRE_DLL REGSVR PATH
  ${If} ${FileExists} "${PATH}"
    ExecWait '"${REGSVR}" /s /u "${PATH}"'
    Push $0
    GetTempFileName $0 "$INSTDIR"
    Delete "$0"
    Rename "${PATH}" "$0"
    Delete /REBOOTOK "$0"
    Pop $0
    SetRebootFlag false
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro PHONECAM_RETIRE_DLL "$SYSDIR\regsvr32.exe" "$INSTDIR\softcam.dll"
  !insertmacro PHONECAM_RETIRE_DLL "$WINDIR\SysWOW64\regsvr32.exe" "$INSTDIR\softcam32\softcam.dll"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; The phone sends video straight to this PC over UDP; without an inbound rule Windows Firewall
  ; drops it silently (the app has no console, so the usual firewall prompt never shows).
  nsExec::Exec 'netsh advfirewall firewall delete rule name="PhoneCam"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="PhoneCam" dir=in action=allow program="$INSTDIR\phonecam.exe" protocol=UDP profile=any'
  ExecWait '"$SYSDIR\regsvr32.exe" /s "$INSTDIR\softcam.dll"'
  ${If} ${FileExists} "$WINDIR\SysWOW64\regsvr32.exe"
    ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "$INSTDIR\softcam32\softcam.dll"'
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'netsh advfirewall firewall delete rule name="PhoneCam"'
  !insertmacro PHONECAM_RETIRE_DLL "$SYSDIR\regsvr32.exe" "$INSTDIR\softcam.dll"
  !insertmacro PHONECAM_RETIRE_DLL "$WINDIR\SysWOW64\regsvr32.exe" "$INSTDIR\softcam32\softcam.dll"
!macroend
