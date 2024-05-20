echo OFF

set SolutionDir=%1
set ProjectDir=%2
set ConfigurationName=%3

if "%ConfigurationName%" == "Debug" (
    set SOURCE_DLL_FILENAME="%SolutionDir%..\target\debug\cs_interop.dll"
)
if "%ConfigurationName%" == "Release" (
    set SOURCE_DLL_FILENAME="%SolutionDir%..\target\release\cs_interop.dll"
)

set TARGET_DLL_FILENAME="%ProjectDir%core_interop.dll"

xcopy %SOURCE_DLL_FILENAME% %TARGET_DLL_FILENAME% /D /Y
