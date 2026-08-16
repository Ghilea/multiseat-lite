@echo off
setlocal

rem CodeQL tracing wrapper: real x64 Release KMDF rebuild only.
rem This script never stages, installs, registers, loads, or binds a driver.
set "MSBUILD=C:\Program Files\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe"
set "PROJECT=%~dp0..\driver\MultiSeatKeyboardFilter.vcxproj"

if not exist "%MSBUILD%" (
  echo ERROR: MSBuild not found at "%MSBUILD%". 1>&2
  exit /b 2
)

"%MSBUILD%" "%PROJECT%" /m /t:Rebuild /p:Configuration=Release /p:Platform=x64 /p:RunCodeAnalysis=true
exit /b %ERRORLEVEL%
