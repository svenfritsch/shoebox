# Install the recognizer with a standalone Python (python-build-standalone),
# OpenCV, numpy and the models on Windows, so `shoebox recognize` finds it
# without anything installed on the computer. The Windows twin of install.sh.
#
#   install.ps1 [-Faces] [-Pets] [-Root library-root]
#
# Without -Root it installs into the folder that holds this script (next to
# shoebox.exe; every drive you recognize then uses it). With one, into
# <library-root>\.shoebox\recognizer\ (travels with that drive).
#
# -Faces (face models, ~40 MB) and -Pets (cat and dog models, ~140 MB) are
# independent; either or both, on top of the Python and OpenCV runtime
# (~200 MB). Without either, you are asked. The Control Panel passes the choice.
# Run it again later to add the other.
#
# Start it from a command prompt as
#   powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1 -Faces
# (the Control Panel does exactly that).
param(
    [switch]$Faces,
    [switch]$Pets,
    [string]$Root
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
# The Control Panel reads our output as UTF-8 (paths may have umlauts).
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }

$Here = $PSScriptRoot
$PbsTag = '20251014'
$Py = '3.12.12'
# SHA-256 of the archive below, from the SHA256SUMS of that release.
$PbsSum = '3C8B9B10A933909C98B9916297E2093B24A9C2ABAA23DF1C2622C2BFE052CB94'

function Fail($Message) {
    Write-Host "ERROR: $Message"
    exit 1
}

# Native programs: their output goes to our output line by line, errors on
# stderr included (Windows PowerShell 5 would turn those into exceptions).
function Invoke-Native {
    param([string]$Program, [string[]]$Arguments)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Program @Arguments 2>&1 | ForEach-Object { Write-Host ([string]$_) }
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
}

try {
    if (-not $Faces -and -not $Pets -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected) {
        $answer = Read-Host 'Find faces (people)? About 40 MB. [Y/n]'
        if ($answer -notmatch '^[nN]') { $Faces = $true }
        $answer = Read-Host 'Find cats and dogs? About 140 MB. [y/N]'
        if ($answer -match '^[yYjJ]') { $Pets = $true }
    }
    if (-not $Faces -and -not $Pets) { Fail 'nothing to install: use -Faces and/or -Pets' }

    if ($Root) {
        if (-not (Test-Path -LiteralPath (Join-Path $Root '.shoebox') -PathType Container)) {
            Fail "$Root has no .shoebox folder (run shoebox scan first)"
        }
        $Dest = Join-Path (Join-Path $Root '.shoebox') 'recognizer'
    } else {
        $Dest = $Here
    }

    # Must match Rust's std::env::consts::{OS, ARCH} (see core/src/recognize.rs).
    # OpenCV has no Windows wheels for ARM, so only 64-bit Intel/AMD.
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64') { Fail "unsupported CPU: $arch (Windows on x86-64 is needed)" }
    $Platform = 'windows-x86_64'
    $Url = "https://github.com/astral-sh/python-build-standalone/releases/download/$PbsTag/cpython-$Py%2B$PbsTag-x86_64-pc-windows-msvc-install_only_stripped.tar.gz"

    # tar.exe is part of Windows 10 (1803) and 11. Use the system's own, not
    # whatever comes first in PATH (a GNU tar mistakes "C:" for a remote host).
    $Tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (-not (Test-Path -LiteralPath $Tar)) { Fail 'tar.exe is missing: this needs Windows 10 (version 1803) or newer' }

    $Tmp = Join-Path ([IO.Path]::GetTempPath()) ('shoebox-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
    try {
        Write-Host "Python $Py for $Platform..."
        $archive = Join-Path $Tmp 'python.tar.gz'
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $archive
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $PbsSum) {
            Fail 'checksum mismatch for the Python download'
        }
        if ((Invoke-Native $Tar @('-xzf', $archive, '-C', $Tmp)) -ne 0) { Fail 'could not unpack Python' }
        $P = Join-Path $Tmp 'python'
        $python = Join-Path $P 'python.exe'
        if (-not (Test-Path -LiteralPath $python)) { Fail 'the Python download has no python.exe' }

        $env:PYTHONDONTWRITEBYTECODE = '1'
        Write-Host 'OpenCV and numpy...'
        # Ready-made wheels only: never compile OpenCV from source.
        $code = Invoke-Native $python @('-m', 'pip', 'install', '--no-cache-dir', '--disable-pip-version-check', '--progress-bar', 'off', '--only-binary', ':all:', 'opencv-python-headless', 'numpy')
        if ($code -ne 0) { Fail 'pip could not install OpenCV; check the internet connection and try again' }
        $code = Invoke-Native $python @('-c', 'import cv2, numpy; print("  OpenCV", cv2.__version__, "numpy", numpy.__version__)')
        if ($code -ne 0) {
            Fail 'OpenCV does not start. Install "Microsoft Visual C++ Redistributable 2015-2022 (x64)" from microsoft.com and try again'
        }

        # Optional: onnxruntime runs the pet embedder faster; OpenCV runs the
        # models itself where there is no wheel.
        Write-Host 'onnxruntime (optional)...'
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $null = & $python -m pip install --no-cache-dir --disable-pip-version-check --only-binary ':all:' onnxruntime 2>&1
        $onnx = $LASTEXITCODE
        $ErrorActionPreference = $previous
        if ($onnx -eq 0) {
            [void](Invoke-Native $python @('-c', 'import onnxruntime; print("  onnxruntime", onnxruntime.__version__)'))
        } else {
            Write-Host '  none for this system; OpenCV runs the pet models instead'
        }

        Write-Host "Copying to $Dest..."
        # Only what running recognizer.py needs. Python for Windows has no
        # symlinks, so a plain copy is fine on NTFS and exFAT. Scripts\ holds
        # launchers (pip.exe) that point back into the temporary folder.
        Get-ChildItem -LiteralPath $P -Recurse -Force -Directory -Filter '__pycache__' | Remove-Item -Recurse -Force
        foreach ($drop in 'include', 'libs', 'tcl', 'Scripts', 'Lib\idlelib', 'Lib\tkinter', 'Lib\turtledemo', 'Lib\ensurepip', 'DLLs\_tkinter.pyd', 'DLLs\tcl86t.dll', 'DLLs\tk86t.dll') {
            $path = Join-Path $P $drop
            if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }
        }
        $runtime = Join-Path $Dest 'runtime'
        New-Item -ItemType Directory -Force -Path $runtime | Out-Null
        $target = Join-Path $runtime $Platform
        $staged = Join-Path $runtime "$Platform.new"
        if (Test-Path -LiteralPath $staged) { Remove-Item -LiteralPath $staged -Recurse -Force }
        Copy-Item -LiteralPath $P -Destination $staged -Recurse
        if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }
        Move-Item -LiteralPath $staged -Destination $target

        # The models are the same on every kind of computer, so they go straight
        # into the folder: ones that are there already (checksum) are not
        # downloaded again, e.g. when only this computer's Python is new.
        Write-Host 'Models...'
        $modelsDir = Join-Path $Dest 'models'
        New-Item -ItemType Directory -Force -Path $modelsDir | Out-Null
        $fetch = @{ Dir = $modelsDir }
        if ($Faces) { $fetch.Faces = $true }
        if ($Pets) { $fetch.Pets = $true }
        & (Join-Path $Here 'fetch-models.ps1') @fetch
        if ($Dest -ne $Here) { Copy-Item -LiteralPath (Join-Path $Here 'recognizer.py') -Destination (Join-Path $Dest 'recognizer.py') -Force }

        Write-Host 'Checking...'
        $bundled = Join-Path $target 'python.exe'
        $workerArgs = @((Join-Path $Dest 'recognizer.py'))
        if ($Pets) { $workerArgs += '--pets' }
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $hello = '' | & $bundled @workerArgs 2>$null | Select-Object -First 1
        $ErrorActionPreference = $previous
        if ("$hello" -notmatch 'shoebox-recognizer') { Fail 'the recognizer did not start' }
        Write-Host "  $hello"
    } finally {
        Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($Root) {
        Write-Host "Done. Run: shoebox recognize `"$Root`""
    } else {
        Write-Host 'Done. Run Recognize in the shoebox launcher for any drive.'
    }
} catch {
    Write-Host "ERROR: $($_.Exception.Message)"
    exit 1
}
