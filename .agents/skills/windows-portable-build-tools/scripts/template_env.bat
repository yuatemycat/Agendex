@echo off
rem ============================================================================
rem  Portable Windows Native Toolchain Environment Setup
rem  Set DEVOPS_ROOT to the base folder where tools are extracted.
rem ============================================================================

if "%DEVOPS_ROOT%"=="" (
    set "DEVOPS_ROOT=%~dp0.."
)

rem 1. Setup MSVC & Windows SDK (if using portable-msvc)
if exist "%DEVOPS_ROOT%\msvc\setup_x64.bat" (
    call "%DEVOPS_ROOT%\msvc\setup_x64.bat"
) else if exist "%DEVOPS_ROOT%\msvc\setup.bat" (
    call "%DEVOPS_ROOT%\msvc\setup.bat"
)

rem 2. Rust & Cargo environment roots
if exist "%DEVOPS_ROOT%\Rustup" set "RUSTUP_HOME=%DEVOPS_ROOT%\Rustup"
if exist "%DEVOPS_ROOT%\Cargo" set "CARGO_HOME=%DEVOPS_ROOT%\Cargo"

rem 3. libclang path for Rust bindgen
if exist "%DEVOPS_ROOT%\Python\Lib\site-packages\clang\native" (
    set "LIBCLANG_PATH=%DEVOPS_ROOT%\Python\Lib\site-packages\clang\native"
)

rem 4. Optional CMake generator
if exist "%DEVOPS_ROOT%\Ninja" set "CMAKE_GENERATOR=Ninja"

rem 5. Prepend tool paths to PATH
set "PATH=%DEVOPS_ROOT%\Git\cmd;%DEVOPS_ROOT%\NodeJS;%DEVOPS_ROOT%\Cargo\bin;%DEVOPS_ROOT%\NASM;%DEVOPS_ROOT%\CMake\bin;%DEVOPS_ROOT%\Ninja;%DEVOPS_ROOT%\Python;%DEVOPS_ROOT%\Python\Scripts;%LIBCLANG_PATH%;%PATH%"

echo [PortableDevOps] Toolchain loaded from %DEVOPS_ROOT%!
