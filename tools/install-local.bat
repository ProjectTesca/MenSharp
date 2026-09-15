@ECHO OFF
SETLOCAL ENABLEEXTENSIONS

:: Installs the MenSharp Unity package (with a locally built compiler binary)
:: into a Unity project as an embedded package - the dev loop for working on
:: MenSharp itself, no CI round-trip needed.
::
::   tools/install-local.bat ~/ALCOM/Projects/MenSharpTest

:: Re-run after changing the compiler or the editor scripts; Unity picks the
:: changes up on focus.

IF "%~1"=="" GOTO USAGE
IF NOT "%~2"=="" GOTO USAGE

SET "project=%~1"
IF NOT EXIST "%project%\Packages\" (
    ECHO error: %project% does not look like a Unity project (no Packages/^) 1>&2
    EXIT /B 1
)

FOR %%I IN ("%~dp0..") DO SET "repo=%%~fI"

ECHO building the compiler (release)...
cargo build --release -p men-sharp --manifest-path "%repo%\Cargo.toml"
IF ERRORLEVEL 1 EXIT /B %ERRORLEVEL%

SET "destination=%project%\Packages\io.tesca.mensharp"
ECHO installing package to %destination%
IF NOT EXIST "%destination%\" MKDIR "%destination%"
IF ERRORLEVEL 1 EXIT /B %ERRORLEVEL%
XCOPY "%repo%\unity\io.tesca.mensharp\*" "%destination%\" /E /I /Y /H /R /K >NUL
IF ERRORLEVEL 1 EXIT /B %ERRORLEVEL%

IF NOT EXIST "%destination%\Compiler~\" MKDIR "%destination%\Compiler~"
IF ERRORLEVEL 1 EXIT /B %ERRORLEVEL%
COPY /Y "%repo%\target\release\men-sharp.exe" "%destination%\Compiler~\men-sharp-windows-x64.exe" >NUL
IF ERRORLEVEL 1 EXIT /B %ERRORLEVEL%

ECHO done - open the project and use MenSharp ^> Compile All
EXIT /B 0

:USAGE
ECHO usage: tools\install-local.bat ^<unity project directory^> 1>&2
EXIT /B 1
