echo OFF

set SolutionDir=%1
set ProjectDir=%2

echo "%ProjectDir%core_interop.dll"
echo "%SolutionDir%..\target\debug\cs_interop.dll"

xcopy "%SolutionDir%..\target\debug\cs_interop.dll" "%ProjectDir%core_interop.dll" /D /Y
