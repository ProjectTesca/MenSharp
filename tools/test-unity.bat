@ECHO OFF
SETLOCAL ENABLEEXTENSIONS

:: Runs the SDK-backed integration suite without opening the Unity UI - the
:: same thing CI does, against the local Unity install.
::
::   tools\test-unity.bat [nunit test filter]
::
:: Environment:
::   UNITY_EDITOR           the Unity 2022.3.22f1 binary (default: Unity Hub's)
::   MENSHARP_VRC_PROJECT   the Worlds project to borrow the SDK from (default:
::                          %USERPROFILE%\ALCOM\Projects\MenSharpTest when
::                          vrc-get is absent)
::
:: Results land in artifacts\unity-tests: results.xml (NUnit) and Editor.log.

FOR %%I IN ("%~dp0..") DO SET "repo=%%~fI"
SET "project=%repo%\tests\unity-project"
SET "artifacts=%repo%\artifacts\unity-tests"
IF DEFINED UNITY_EDITOR (
    SET "unity=%UNITY_EDITOR%"
) ELSE (
    SET "unity=%ProgramFiles%\Unity\Hub\Editor\2022.3.22f1\Editor\Unity.exe"
)

SET "filter=%~1"
SET "packages=%project%\Packages"
SET "mensharp=%packages%\io.tesca.mensharp"
IF NOT EXIST "%packages%\" (
    ECHO error: %project% does not look like a Unity project (no Packages/^) 1>&2
    EXIT /B 1
)

IF NOT EXIST "%unity%" (
    ECHO Unity editor not found at %unity%; set UNITY_EDITOR 1>&2
    EXIT /B 1
)

IF NOT "%MENSHARP_VRC_PROJECT%"=="" GOTO STAGE_PACKAGE
WHERE vrc-get >NUL 2>&1
IF NOT ERRORLEVEL 1 GOTO STAGE_PACKAGE
SET "default_donor=%USERPROFILE%\ALCOM\Projects\MenSharpTest"
IF EXIST "%default_donor%\Packages\com.vrchat.worlds\" SET "MENSHARP_VRC_PROJECT=%default_donor%"

:STAGE_PACKAGE
ECHO building the compiler (release)...
cargo build --release -p men-sharp --manifest-path "%repo%\Cargo.toml"
IF ERRORLEVEL 1 EXIT /B 1

IF EXIST "%mensharp%\" RD /S /Q "%mensharp%"
IF ERRORLEVEL 1 EXIT /B 1
MKDIR "%mensharp%"
IF ERRORLEVEL 1 EXIT /B 1
XCOPY "%repo%\unity\io.tesca.mensharp\*" "%mensharp%\" /E /I /Y /H /R /K >NUL
IF ERRORLEVEL 2 EXIT /B 1
IF NOT EXIST "%mensharp%\Compiler~\" MKDIR "%mensharp%\Compiler~"
IF ERRORLEVEL 1 EXIT /B 1
COPY /Y "%repo%\target\release\men-sharp.exe" "%mensharp%\Compiler~\men-sharp-windows-x64.exe" >NUL
IF ERRORLEVEL 1 EXIT /B 1

IF "%MENSHARP_SKIP_VPM_RESOLVE%"=="1" GOTO SDK_READY
WHERE vrc-get >NUL 2>&1
IF NOT ERRORLEVEL 1 GOTO RESOLVE_SDK
IF "%MENSHARP_VRC_PROJECT%"=="" (
    ECHO vrc-get is not installed and MENSHARP_VRC_PROJECT is unset 1>&2
    ECHO set MENSHARP_VRC_PROJECT to an ALCOM/VCC Worlds project 1>&2
    EXIT /B 1
)
SET "donor=%MENSHARP_VRC_PROJECT%"
FOR %%N IN (com.vrchat.base com.vrchat.worlds) DO (
    IF NOT EXIST "%donor%\Packages\%%N\" (
        ECHO missing %donor%\Packages\%%N 1>&2
        EXIT /B 1
    )
    IF EXIST "%packages%\%%N\" RD /S /Q "%packages%\%%N"
    IF ERRORLEVEL 1 EXIT /B 1
    MKLINK /J "%packages%\%%N" "%donor%\Packages\%%N" >NUL
    IF ERRORLEVEL 1 EXIT /B 1
)
GOTO SDK_READY

:RESOLVE_SDK
vrc-get resolve --project "%project%"
IF ERRORLEVEL 1 EXIT /B 1

:SDK_READY
IF EXIST "%artifacts%\" RD /S /Q "%artifacts%"
IF ERRORLEVEL 1 EXIT /B 1
MKDIR "%artifacts%"
IF ERRORLEVEL 1 EXIT /B 1

IF NOT "%filter%"=="" GOTO RUN_FILTERED
"%unity%" -batchmode -nographics -projectPath "%project%" -runTests -testPlatform EditMode -assemblyNames ProjectTesca.MenSharp.IntegrationTests -testResults "%artifacts%\results.xml" -logFile "%artifacts%\Editor.log"
SET "status=%ERRORLEVEL%"
GOTO CHECK_RESULTS

:RUN_FILTERED
"%unity%" -batchmode -nographics -projectPath "%project%" -runTests -testPlatform EditMode -assemblyNames ProjectTesca.MenSharp.IntegrationTests -testResults "%artifacts%\results.xml" -logFile "%artifacts%\Editor.log" -testFilter "%filter%"
SET "status=%ERRORLEVEL%"

:CHECK_RESULTS
IF NOT EXIST "%artifacts%\results.xml" GOTO NO_RESULTS
FOR %%I IN ("%artifacts%\results.xml") DO IF %%~zI EQU 0 GOTO NO_RESULTS
FINDSTR /R /C:"<test-run .*result=.*Passed" "%artifacts%\results.xml" >NUL
IF ERRORLEVEL 1 GOTO TESTS_FAILED
ECHO Unity tests passed: %artifacts%\results.xml
EXIT /B 0

:NO_RESULTS
ECHO Unity produced no test results (exit %status%); see %artifacts%\Editor.log 1>&2
IF EXIST "%artifacts%\Editor.log" FINDSTR /R /C:"error CS" /C:"Exception" /C:"Assertion" "%artifacts%\Editor.log"
EXIT /B 1

:TESTS_FAILED
ECHO Unity tests did not all pass (exit %status%); see %artifacts%\results.xml 1>&2
FINDSTR /C:"<test-case " "%artifacts%\results.xml"
EXIT /B 1

:USAGE
ECHO usage: tools\test-unity.bat [nunit test filter] 1>&2
EXIT /B 1
