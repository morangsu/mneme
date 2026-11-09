@echo off
rem One-click rebuild: Rust arm64 cdylib -> jniLibs -> debug APK.
rem Requires JDK 21 (AGP 8.7 rejects JDK 27) and Android NDK r30 at
rem D:\scoop\apps\android-ndk\current (paths are set in ..\.cargo\config.toml).
setlocal

set "JAVA_HOME=D:\scoop\apps\corretto21-jdk\current"
set "PATH=%JAVA_HOME%\bin;%PATH%"
set "ROOT=%~dp0.."

pushd "%ROOT%"
echo [1/3] cargo build arm64 ...
cargo build --target aarch64-linux-android --release -p mneme-ffi
if errorlevel 1 goto :err

echo [2/3] copy libmneme_ffi.so to jniLibs ...
copy /Y "target\aarch64-linux-android\release\libmneme_ffi.so" "android\app\src\main\jniLibs\arm64-v8a\libmneme_ffi.so" >nul
if errorlevel 1 goto :err
popd

echo [3/3] gradle assembleDebug ...
pushd "%~dp0"
call gradlew.bat :app:assembleDebug --console=plain
if errorlevel 1 goto :err
popd

echo.
echo OK: android\app\build\outputs\apk\debug\app-debug.apk
echo install: adb install -r android\app\build\outputs\apk\debug\app-debug.apk
exit /b 0

:err
echo.
echo BUILD FAILED
exit /b 1
