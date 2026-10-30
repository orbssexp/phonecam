; Register / unregister the softcam DirectShow filter ("PhoneCam" camera). perMachine install
; runs elevated, which regsvr32 for HKLM needs. Resources land next to the exe ($INSTDIR).
!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$SYSDIR\regsvr32.exe" /s "$INSTDIR\softcam.dll"'
  IfFileExists "$WINDIR\SysWOW64\regsvr32.exe" 0 +2
  ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "$INSTDIR\softcam32\softcam.dll"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$SYSDIR\regsvr32.exe" /s /u "$INSTDIR\softcam.dll"'
  IfFileExists "$WINDIR\SysWOW64\regsvr32.exe" 0 +2
  ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s /u "$INSTDIR\softcam32\softcam.dll"'
!macroend
