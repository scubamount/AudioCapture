# Build the original C++ recorder

Build from the repository root. Install Visual Studio 2022 with **Desktop development with C++** and a Windows SDK. Use CMake 3.21 or later for the Visual Studio 2022 generator.

## Set up vcpkg

Clone vcpkg if you need a local copy. Run its bootstrap script.

```powershell
git clone https://github.com/microsoft/vcpkg.git C:\vcpkg
& C:\vcpkg\bootstrap-vcpkg.bat
```

Set `VCPKG_ROOT` to your vcpkg directory. CMake uses the root `vcpkg.json` manifest to install dependencies. The manifest lists Opus, FLAC, and nlohmann-json. FLAC also brings in libogg.

## Build and stage

The batch script expects CMake at `C:\Program Files\CMake\bin\cmake.exe`. For the batch script, use a vcpkg path without spaces.

```powershell
$env:VCPKG_ROOT = 'C:\vcpkg'
.\build.bat
```

The script configures x64 with `x64-windows-static`. It builds Release and stages `package\AudioCapture.exe`. It copies the C++ user guide to `package\README.md`. The executable uses static codecs and the static C++ runtime. The release workflow also installs dependencies in the overlay triplet `x64-windows-static-mt`. The batch build itself uses the standard triplet.

To create a ZIP like the C++ release, run this command after staging.

```powershell
Compress-Archive -Path .\package\AudioCapture.exe, .\package\README.md -DestinationPath .\AudioCapture-latest-windows-x64.zip
```

## Build manually

Use CMake on `PATH` for a different CMake installation. If the existing build directory uses a different generator or triplet, select a new directory.

```powershell
$env:VCPKG_ROOT = 'C:\vcpkg'
cmake -S . -B build -G "Visual Studio 17 2022" -A x64 "-DCMAKE_TOOLCHAIN_FILE=$env:VCPKG_ROOT/scripts/buildsystems/vcpkg.cmake" -DVCPKG_TARGET_TRIPLET=x64-windows-static
cmake --build build --config Release --target AudioCapture
```

The executable is `build\bin\Release\AudioCapture.exe`. For a debug build, change `--config Release` to `--config Debug`. Find its executable in `build\bin\Debug`.
