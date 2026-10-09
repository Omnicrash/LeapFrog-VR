; LeapFrog installer (NSIS, Modern UI). The wizard copies the driver folder; the SteamVR and
; service steps are leapfrog-register.exe's, run quietly (it elevates itself once for the service).
; build.sh --package compiles this: makensis -DVERSION=... -DSRC=setup -DDIST=... -DOUT=... setup/leapfrog.nsi

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

Name "LeapFrog ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\LeapFrog\driver"
InstallDirRegKey HKCU "Software\LeapFrog" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "LeapFrog ${VERSION}"

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\LeapFrog"

!define MUI_ICON "${SRC}/leapfrog.ico"
!define MUI_UNICON "${SRC}/leapfrog.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "LeapFrog ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "Ultraleap hand tracking for your SteamVR controllers.$\r$\n$\r$\nThis installs the LeapFrog driver, registers it with SteamVR and prepares the Ultraleap tracking service. One step needs administrator rights, for the service.$\r$\n$\r$\nClose SteamVR before continuing."
!define MUI_FINISHPAGE_TEXT "LeapFrog is installed. Start SteamVR: your controllers behave as before until you put one down, and the LeapFrog tab in the dashboard holds the settings."

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${SRC}/../LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Refuses while vrserver.exe runs: the driver DLL would be in use and the registration ignored.
; find exits 0 when tasklist listed the process. One body for the installer and the uninstaller.
!macro CheckSteamVRBody WHAT
    nsExec::ExecToStack '"$SYSDIR\cmd.exe" /c ""$SYSDIR\tasklist.exe" /FI "IMAGENAME eq vrserver.exe" /NH | "$SYSDIR\find.exe" /I "vrserver.exe""'
    Pop $0
    Pop $1
    ${If} $0 == 0
        MessageBox MB_OK|MB_ICONSTOP "SteamVR is running. Close it and run the ${WHAT} again."
        Abort
    ${EndIf}
!macroend

Function CheckSteamVR
    !insertmacro CheckSteamVRBody "installer"
FunctionEnd

Function un.CheckSteamVR
    !insertmacro CheckSteamVRBody "uninstaller"
FunctionEnd

; Appends the engine's output to %TEMP%\LeapFrog-setup.log, for when something goes wrong.
!macro WriteLogBody
    Exch $R0
    Push $R1
    FileOpen $R1 "$TEMP\LeapFrog-setup.log" a
    FileSeek $R1 0 END
    FileWrite $R1 "$R0$\r$\n"
    FileClose $R1
    Pop $R1
    Pop $R0
!macroend

Function WriteLog
    !insertmacro WriteLogBody
FunctionEnd

Function un.WriteLog
    !insertmacro WriteLogBody
FunctionEnd

Function .onInit
    Call CheckSteamVR
FunctionEnd

Section "LeapFrog" SecMain
    SectionIn RO
    SetOutPath "$INSTDIR"
    File /r "${DIST}/*"
    DetailPrint "Registering with SteamVR and preparing the tracking service..."
    nsExec::ExecToStack '"$INSTDIR\leapfrog-register.exe" register --no-apps-entry'
    Pop $0
    Pop $1
    DetailPrint $1
    Push "register: exit $0$\r$\n$1"
    Call WriteLog
    ${If} $0 != 0
        MessageBox MB_OK|MB_ICONEXCLAMATION "The SteamVR registration step failed (code $0). The files are in place; run leapfrog-register.exe in the install folder to retry."
    ${EndIf}
    WriteUninstaller "$INSTDIR\uninstall.exe"
    WriteRegStr HKCU "Software\LeapFrog" "InstallDir" "$INSTDIR"
    ; The Settings > Apps entry is the wizard's (register ran with --no-apps-entry).
    WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "LeapFrog"
    WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Omnicrash"
    WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\leapfrog-register.exe"
    WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Function un.onInit
    Call un.CheckSteamVR
FunctionEnd

Section "Uninstall"
    StrCpy $1 ""
    ${IfNot} ${Silent}
        MessageBox MB_YESNO|MB_ICONQUESTION "Also remove the LeapFrog settings from steamvr.vrsettings (hand placement, switching)?" IDNO +2
        StrCpy $1 "--purge-settings"
    ${EndIf}
    DetailPrint "Unregistering from SteamVR..."
    nsExec::ExecToStack '"$INSTDIR\leapfrog-register.exe" unregister $1'
    Pop $0
    Pop $2
    DetailPrint $2
    Push "unregister $1: exit $0$\r$\n$2"
    Call un.WriteLog
    ; Keep the files while SteamVR may still point at them: a registration of a deleted folder
    ; would stay behind.
    ${If} $0 != 0
        ${IfNot} ${Silent}
            MessageBox MB_OK|MB_ICONSTOP "Unregistering from SteamVR failed (code $0), so the files stay in place. Details: $TEMP\LeapFrog-setup.log"
        ${EndIf}
        Abort
    ${EndIf}
    RMDir /r "$INSTDIR"
    DeleteRegKey HKCU "${UNINST_KEY}"
    DeleteRegKey HKCU "Software\LeapFrog"
SectionEnd
