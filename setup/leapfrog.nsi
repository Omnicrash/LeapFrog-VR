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
Function CheckSteamVR
    nsExec::ExecToStack 'tasklist.exe /FI "IMAGENAME eq vrserver.exe" /NH'
    Pop $0
    Pop $1
    ${If} $1 != ""
        StrCpy $2 $1 ; search for the process name in the output
        Push $1
        Push "vrserver.exe"
        Call StrContains
        Pop $3
        ${If} $3 == "1"
            MessageBox MB_OK|MB_ICONSTOP "SteamVR is running. Close it and run the installer again."
            Abort
        ${EndIf}
    ${EndIf}
FunctionEnd

; Pushes 1 when the first string contains the second (case as given), else 0.
Function StrContains
    Exch $R1 ; needle
    Exch
    Exch $R0 ; haystack
    Push $R2
    Push $R3
    Push $R4
    StrLen $R2 $R1
    StrCpy $R3 0
    StrCpy $R4 0
    loop:
        StrCpy $R4 $R0 $R2 $R3
        StrCmp $R4 "" notfound
        StrCmp $R4 $R1 found
        IntOp $R3 $R3 + 1
        Goto loop
    found:
        StrCpy $R0 1
        Goto done
    notfound:
        StrCpy $R0 0
    done:
    Pop $R4
    Pop $R3
    Pop $R2
    Pop $R1
    Exch $R0
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
    nsExec::ExecToStack '"$INSTDIR\leapfrog-register.exe" register'
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
    ; The Settings > Apps entry: the wizard's uninstaller, over the values leapfrog-register.exe wrote.
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

Function un.CheckSteamVR
    nsExec::ExecToStack 'tasklist.exe /FI "IMAGENAME eq vrserver.exe" /NH'
    Pop $0
    Pop $1
    Push $1
    Push "vrserver.exe"
    Call un.StrContains
    Pop $3
    ${If} $3 == "1"
        MessageBox MB_OK|MB_ICONSTOP "SteamVR is running. Close it and run the uninstaller again."
        Abort
    ${EndIf}
FunctionEnd

Function un.StrContains
    Exch $R1
    Exch
    Exch $R0
    Push $R2
    Push $R3
    Push $R4
    StrLen $R2 $R1
    StrCpy $R3 0
    StrCpy $R4 0
    loop:
        StrCpy $R4 $R0 $R2 $R3
        StrCmp $R4 "" notfound
        StrCmp $R4 $R1 found
        IntOp $R3 $R3 + 1
        Goto loop
    found:
        StrCpy $R0 1
        Goto done
    notfound:
        StrCpy $R0 0
    done:
    Pop $R4
    Pop $R3
    Pop $R2
    Pop $R1
    Exch $R0
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
    RMDir /r "$INSTDIR"
    DeleteRegKey HKCU "${UNINST_KEY}"
    DeleteRegKey HKCU "Software\LeapFrog"
SectionEnd
