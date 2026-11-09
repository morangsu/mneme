@echo off
rem 把公共数据仓库（../mneme-data-public，或本仓库 domains/）里的领域包同步进 APK assets。
rem 只读拷贝：assets 里的文件不要手改，改动一律回 domains/ 再跑这个脚本。
setlocal
set "ROOT=%~dp0.."
set "SRC=%ROOT%\..\mneme-data-public"
if not exist "%SRC%" set "SRC=%ROOT%\domains"
if not exist "%SRC%" (
  echo [error] 找不到数据目录：%SRC%
  exit /b 1
)
set "DST=%ROOT%\android\app\src\main\assets\domains"
if not exist "%DST%" mkdir "%DST%"
copy /Y "%SRC%\*.toml" "%DST%\" >nul
copy /Y "%SRC%\sample-herbs.csv" "%DST%\materia-medica.csv" >nul
echo synced from %SRC%
dir /b "%DST%"
