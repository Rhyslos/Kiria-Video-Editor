@echo off
rem Measures the audio engine designs and writes results.md next to this file.
echo Measuring, about two minutes...
"%~dp0audio-bench.exe" > "%~dp0results.md"
echo Done: results.md
pause
