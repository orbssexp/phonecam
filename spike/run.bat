@echo off
rem Usage: run.bat [spike-rx args]   (after: cargo build --release, see README)
set FF=%~dp0vendor\ffmpeg\ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1\bin
set SC=%~dp0vendor\softcam-src\src\softcam\dist\bin\x64
set PATH=%FF%;%SC%;%PATH%
cd /d %~dp0
target\release\spike-rx.exe %*
