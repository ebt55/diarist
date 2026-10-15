; meet-assist installer hooks for Tauri's NSIS template (docs/01-architecture.md,
; "Install layout"; docs/research/06-adjudication.md, point C).
;
; The data root is %LOCALAPPDATA%\app.meetassist.desktop unless the user moved
; it in Settings; then HKCU\Software\meet-assist\DataRoot names it. That key is
; ours, not one of the identifier keys the template deletes, so it is still
; there when POSTUNINSTALL runs.
;
; Uninstall (never during an update, $UpdateMode = 1):
; - "Delete app data" ticked: the template has already removed
;   %LOCALAPPDATA%\app.meetassist.desktop. For a moved data root this hook also
;   removes meet-assist's items there (database, audio, engine, models,
;   manifests, logs, WebView profile, window state), then the folder itself if
;   that leaves it empty, and finally the registry key. Other files in a moved
;   folder are never touched.
; - Unticked: it asks "Remove the downloaded engine and models (N MB)?
;   Transcripts and recordings are kept" and removes only engine\ and models\.
;
; Silent uninstall (/S) keeps everything unless told otherwise:
;   /DELETEAPPDATA   as if "Delete app data" were ticked
;   /REMOVEMODELS    answer Yes to the engine-and-models question
; A junction or symlink is removed as a link; its target is never followed.

; Removes a file, a folder tree, or just the link when PATH is a junction or
; symlink (FILE_ATTRIBUTE_REPARSE_POINT). Uses $8 and $9.
!macro MA_RemoveItem PATH
  System::Call 'kernel32::GetFileAttributesW(w "${PATH}") i .r9'
  ${If} $9 <> -1
    IntOp $8 $9 & 0x400
    ${If} $8 <> 0
      RMDir "${PATH}"
      Delete "${PATH}"
    ${Else}
      IntOp $8 $9 & 0x10
      ${If} $8 <> 0
        RMDir /r "${PATH}"
      ${Else}
        Delete "${PATH}"
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

; Everything meet-assist keeps under a data root (the data-folder move in
; crates/core/src/models/relocate.rs handles the same items).
!macro MA_RemoveDataItems ROOT
  !insertmacro MA_RemoveItem "${ROOT}\meet-assist.db"
  !insertmacro MA_RemoveItem "${ROOT}\meet-assist.db-wal"
  !insertmacro MA_RemoveItem "${ROOT}\meet-assist.db-shm"
  !insertmacro MA_RemoveItem "${ROOT}\audio"
  !insertmacro MA_RemoveItem "${ROOT}\engine"
  !insertmacro MA_RemoveItem "${ROOT}\models"
  !insertmacro MA_RemoveItem "${ROOT}\manifests"
  !insertmacro MA_RemoveItem "${ROOT}\logs"
  !insertmacro MA_RemoveItem "${ROOT}\EBWebView"
  !insertmacro MA_RemoveItem "${ROOT}\.window-state.json"
  !insertmacro MA_RemoveItem "${ROOT}\moved-from.json"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; /DELETEAPPDATA: what ticking "Delete app data" does, for silent uninstalls.
  ClearErrors
  ${GetOptions} $CMDLINE "/DELETEAPPDATA" $R9
  ${IfNot} ${Errors}
    StrCpy $DeleteAppDataCheckboxState 1
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; $R7: the data root; $R6: 1 when it was moved (registry key set).
    StrCpy $R6 0
    ReadRegStr $R7 HKCU "Software\meet-assist" "DataRoot"
    ${If} $R7 == ""
      SetShellVarContext current
      StrCpy $R7 "$LOCALAPPDATA\${BUNDLEID}"
    ${Else}
      StrCpy $R6 1
    ${EndIf}

    ${If} $DeleteAppDataCheckboxState = 1
      ${If} $R6 = 1
        DetailPrint "Removing meet-assist data from $R7"
        !insertmacro MA_RemoveDataItems "$R7"
        ; Only when nothing else is left in it.
        RMDir "$R7"
      ${EndIf}
      DeleteRegValue HKCU "Software\meet-assist" "DataRoot"
      DeleteRegKey /ifempty HKCU "Software\meet-assist"
    ${Else}
      ${If} ${FileExists} "$R7\engine\*.*"
      ${OrIf} ${FileExists} "$R7\models\*.*"
        ${GetSize} "$R7\engine" "/S=0M" $R1 $R2 $R3
        ${GetSize} "$R7\models" "/S=0M" $R4 $R2 $R3
        IntOp $R1 $R1 + $R4
        ClearErrors
        ${GetOptions} $CMDLINE "/REMOVEMODELS" $R9
        ${IfNot} ${Errors}
          Goto ma_remove_models
        ${EndIf}
        MessageBox MB_YESNO|MB_ICONQUESTION "Remove the downloaded engine and models ($R1 MB)?$\r$\n$\r$\nTranscripts and recordings are kept in$\r$\n$R7" /SD IDNO IDYES ma_remove_models
        Goto ma_keep_models
        ma_remove_models:
          DetailPrint "Removing the engine and models from $R7"
          !insertmacro MA_RemoveItem "$R7\engine"
          !insertmacro MA_RemoveItem "$R7\models"
        ma_keep_models:
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
