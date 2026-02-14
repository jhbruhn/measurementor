; nsis-extra.nsi — included by Tauri's generated NSIS installer script.
;
; Problem: Windows resolves DLLs by searching the directory containing the .exe
; first, then directories on PATH.  Tauri places resources in a sub-folder
; ($INSTDIR\resources\), which is NOT in that search path.
;
; Solution: copy the bundled FFmpeg / Tesseract DLLs from the resources folder
; to $INSTDIR itself so Windows finds them next to the executable.
;
; On uninstall the same DLLs are removed from $INSTDIR.

!macro customInstall
  ; Copy every .dll from the resources root to the install directory.
  ; build.rs placed the DLLs at resources\*.dll (destination "." in tauri.conf.json).
  SetOutPath "$INSTDIR"
  CopyFiles /SILENT "$INSTDIR\resources\*.dll" "$INSTDIR"
!macroend

!macro customUninstall
  ; Remove the DLLs that customInstall copied to $INSTDIR.
  ; Uses a wildcard per known prefix to avoid deleting unrelated user files.
  ; FFmpeg
  Delete "$INSTDIR\avutil*.dll"
  Delete "$INSTDIR\avformat*.dll"
  Delete "$INSTDIR\avcodec*.dll"
  Delete "$INSTDIR\avfilter*.dll"
  Delete "$INSTDIR\avdevice*.dll"
  Delete "$INSTDIR\swscale*.dll"
  Delete "$INSTDIR\swresample*.dll"
  ; Tesseract + Leptonica
  Delete "$INSTDIR\tesseract*.dll"
  Delete "$INSTDIR\leptonica*.dll"
  ; Leptonica image-format transitive deps
  Delete "$INSTDIR\jpeg*.dll"
  Delete "$INSTDIR\gif*.dll"
  Delete "$INSTDIR\libpng*.dll"
  Delete "$INSTDIR\tiff*.dll"
  Delete "$INSTDIR\webp*.dll"
  Delete "$INSTDIR\zlib*.dll"
  Delete "$INSTDIR\archive*.dll"
  Delete "$INSTDIR\libcurl*.dll"
  Delete "$INSTDIR\liblzma*.dll"
  Delete "$INSTDIR\bz2*.dll"
!macroend
