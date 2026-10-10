# Download the ONNX models the recognizer needs (OpenCV model zoo, Apache 2.0)
# and check their SHA-256. The Windows twin of fetch-models.sh; the checksums
# must be the same (core/tests/recognize.rs checks that).
#
#   fetch-models.ps1 [-Faces] [-Pets] [-Dir folder]   (default folder: recognizer\models)
#
# -Faces fetches the face models (~40 MB), -Pets the two pet models (~140 MB).
# Without either, both; SHOEBOX_NO_PETS=1 leaves out the pets.
param(
    [switch]$Faces,
    [switch]$Pets,
    [string]$Dir
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # progress bars make Invoke-WebRequest very slow in Windows PowerShell 5
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

if (-not $Faces -and -not $Pets) {
    $Faces = $true
    if (-not $env:SHOEBOX_NO_PETS) { $Pets = $true }
}
if (-not $Dir) { $Dir = Join-Path $PSScriptRoot 'models' }
$Base = 'https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models'
New-Item -ItemType Directory -Force -Path $Dir | Out-Null

function Get-Model($Name, $Path, $Sum) {
    $file = Join-Path $Dir $Name
    if ((Test-Path -LiteralPath $file) -and ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -eq $Sum)) {
        Write-Host "have $Name"
        return
    }
    $part = "$file.part"
    Invoke-WebRequest -UseBasicParsing -Uri "$Base/$Path/$Name" -OutFile $part
    if ((Get-FileHash -LiteralPath $part -Algorithm SHA256).Hash -ne $Sum) {
        Remove-Item -LiteralPath $part -Force
        throw "checksum mismatch for $Name"
    }
    Move-Item -LiteralPath $part -Destination $file -Force
    Write-Host "fetched $Name"
}

if ($Faces) {
    Get-Model 'face_detection_yunet_2023mar.onnx' 'face_detection_yunet' '8F2383E4DD3CFBB4553EA8718107FC0423210DC964F9F4280604804ED2552FA4'
    Get-Model 'face_recognition_sface_2021dec.onnx' 'face_recognition_sface' '0BA9FBFA01B5270C96627C4EF784DA859931E02F04419C829E83484087C34E79'
}
if ($Pets) {
    Get-Model 'object_detection_yolox_2022nov.onnx' 'object_detection_yolox' 'C5C2D13E59AE883E6AF3B45DAEA64AF4833A4951C92D116EC270D9DDBE998063'
    Get-Model 'image_classification_ppresnet50_2022jan.onnx' 'image_classification_ppresnet' 'AD5486B0DE6C2171EA4D28C734C2FB7C5F64FCDBD97180A0EF515CF4B766A405'
}
