Param(
    [string]$Version = $env:LIBEWF_VERSION,
    [string]$ExpectedSha256 = $env:LIBEWF_SHA256,
    [string]$Bzip2Ref = $env:BZIP2_REF,
    [string]$VsToolsRef = $env:VSTOOLS_REF,
    [string]$OutputDir = "deps\libewf\dist"
)

$ErrorActionPreference = "Stop"

if (-not $Version) {
    throw "LIBEWF_VERSION or -Version must be set"
}
if (-not $ExpectedSha256) {
    throw "LIBEWF_SHA256 or -ExpectedSha256 must be set"
}
if (-not $Bzip2Ref) {
    throw "BZIP2_REF or -Bzip2Ref must be set"
}
if (-not $VsToolsRef) {
    throw "VSTOOLS_REF or -VsToolsRef must be set"
}

$RepoRoot = (Resolve-Path ".").Path
$OutputPath = Join-Path $RepoRoot $OutputDir
$LibDir = Join-Path $OutputPath "lib"
$BinDir = Join-Path $OutputPath "bin"
$LicenseDir = Join-Path $OutputPath "licenses"
$BuildRoot = Join-Path $RepoRoot "deps\libewf-build"

function Copy-RequiredFile {
    Param(
        [string]$SourceDir,
        [string]$DestinationDir,
        [string]$Name
    )

    $SourcePath = Join-Path $SourceDir $Name
    if (-not (Test-Path $SourcePath)) {
        throw "Missing required file: $SourcePath"
    }
    New-Item -ItemType Directory -Force -Path $DestinationDir | Out-Null
    Copy-Item $SourcePath -Destination $DestinationDir -Force
}

if ((Test-Path (Join-Path $LibDir "libewf.lib")) -and (Test-Path (Join-Path $BinDir "libewf.dll"))) {
    Write-Host "Using cached libewf build in $OutputPath"
    exit 0
}

Remove-Item -Recurse -Force $BuildRoot -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $BuildRoot | Out-Null
New-Item -ItemType Directory -Force -Path $LibDir, $BinDir, $LicenseDir | Out-Null

$Archive = Join-Path $BuildRoot "libewf.tar.gz"
$Url = "https://github.com/libyal/libewf/archive/refs/tags/$Version.tar.gz"

Write-Host "Downloading libewf $Version from $Url"
Invoke-WebRequest -Uri $Url -OutFile $Archive

$ActualSha256 = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
if ($ActualSha256 -ne $ExpectedSha256.ToLowerInvariant()) {
    throw "libewf archive SHA-256 mismatch: expected $ExpectedSha256, got $ActualSha256"
}

tar -xzf $Archive -C $BuildRoot
if ($LASTEXITCODE -ne 0) {
    throw "Failed to extract libewf archive"
}

$SourceDir = Get-ChildItem -Path $BuildRoot -Directory -Filter "libewf-*" | Select-Object -First 1
if (-not $SourceDir) {
    throw "Unable to locate extracted libewf source directory"
}

$SiblingRoot = $SourceDir.Parent.FullName
$Bzip2Dir = Join-Path $SiblingRoot "bzip2"
if (-not (Test-Path $Bzip2Dir)) {
    git clone https://github.com/joachimmetz/bzip2.git $Bzip2Dir
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to clone bzip2"
    }
    git -C $Bzip2Dir checkout $Bzip2Ref
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to check out bzip2 ref $Bzip2Ref"
    }
}
$VsToolsDir = Join-Path $SiblingRoot "vstools"
if (-not (Test-Path $VsToolsDir)) {
    git clone https://github.com/libyal/vstools.git $VsToolsDir
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to clone vstools"
    }
    git -C $VsToolsDir checkout $VsToolsRef
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to check out vstools ref $VsToolsRef"
    }
}

Push-Location $SourceDir.FullName
try {
    if (Test-Path ".\syncwinflexbison.ps1") {
        .\syncwinflexbison.ps1
    }
    if (Test-Path ".\synclibs.ps1") {
        .\synclibs.ps1
    }
    if (Test-Path ".\synczlib.ps1") {
        $ZlibSyncScript = Join-Path $SourceDir.FullName "synczlib.ps1"
        $ZlibSyncContent = Get-Content -Raw $ZlibSyncScript
        $ZlibOriginalUrl = '$Url = "https://zlib.net/zlib131.zip"'
        $ZlibPinnedUrl = '$Url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib131.zip"'
        if ([regex]::Matches($ZlibSyncContent, [regex]::Escape($ZlibOriginalUrl)).Count -ne 1) {
            throw "Unable to patch libewf synczlib.ps1 URL deterministically"
        }
        $ZlibSyncContent = $ZlibSyncContent.Replace($ZlibOriginalUrl, $ZlibPinnedUrl)

        $ZlibDownloadLine = 'Invoke-WebRequest -Uri ${Url} -OutFile ${Filename}'
        $ZlibDownloadReplacement = @'
Invoke-WebRequest -Uri ${Url} -OutFile ${Filename}

$ExpectedSha256 = "72af66d44fcc14c22013b46b814d5d2514673dda3d115e64b690c1ad636e7b17"
$ActualSha256 = (Get-FileHash -Algorithm SHA256 ${Filename}).Hash.ToLowerInvariant()
If (${ActualSha256} -ne ${ExpectedSha256})
{
        Write-Host "zlib archive SHA-256 mismatch: expected ${ExpectedSha256}, got ${ActualSha256}" -foreground Red

        Exit 1
}
'@
        if ([regex]::Matches($ZlibSyncContent, [regex]::Escape($ZlibDownloadLine)).Count -ne 1) {
            throw "Unable to patch libewf synczlib.ps1 checksum block deterministically"
        }
        $ZlibSyncContent = $ZlibSyncContent.Replace($ZlibDownloadLine, $ZlibDownloadReplacement)
        Set-Content -Path $ZlibSyncScript -Value $ZlibSyncContent -Encoding UTF8

        .\synczlib.ps1
    }
    if (Test-Path ".\autogen.ps1") {
        .\autogen.ps1
    }

    $Python = (Get-Command python).Source
    $PythonPath = Split-Path $Python
    $BuildScript = Join-Path $SourceDir.FullName "build.ps1"
    if (-not (Test-Path $BuildScript)) {
        throw "Missing libewf Windows build script: $BuildScript"
    }
    $BuildScriptContent = Get-Content -Raw $BuildScript

    $MsvscppConvertPath = Join-Path $VsToolsDir "vstools\scripts\msvscpp_convert.py"
    if (-not (Test-Path $MsvscppConvertPath)) {
        throw "Missing pinned vstools converter: $MsvscppConvertPath"
    }
    $MsvscppConvertOriginal = '$MSVSCppConvert = "${VSToolsPath}\scripts\msvscpp-convert.py"'
    if ([regex]::Matches($BuildScriptContent, [regex]::Escape($MsvscppConvertOriginal)).Count -ne 1) {
        throw "Unable to patch libewf build.ps1 msvscpp converter path deterministically"
    }
    $MsvscppConvertReplacement = '$MSVSCppConvert = "' + $MsvscppConvertPath + '"'
    $BuildScriptContent = $BuildScriptContent.Replace($MsvscppConvertOriginal, $MsvscppConvertReplacement)

    $MSBuildOptionsOriginal = '$MSBuildOptions = "/verbosity:quiet /target:Build /property:Configuration=${Configuration},Platform=${Platform}"'
    if ([regex]::Matches($BuildScriptContent, [regex]::Escape($MSBuildOptionsOriginal)).Count -ne 1) {
        throw "Unable to patch libewf build.ps1 MSBuild target deterministically"
    }
    $MSBuildOptionsReplacement = '$MSBuildOptions = "/verbosity:quiet /target:libewf /property:Configuration=${Configuration},Platform=${Platform}"'
    $BuildScriptContent = $BuildScriptContent.Replace($MSBuildOptionsOriginal, $MSBuildOptionsReplacement)

    $VsToolsUpdatePattern = '(?s)Else\s*\{\s*Push-Location "\$\{VSToolsPath\}".*?Pop-Location\s*\}\s*\}'
    $VsToolsUpdateMatches = [regex]::Matches($BuildScriptContent, $VsToolsUpdatePattern)
    if ($VsToolsUpdateMatches.Count -ne 1) {
        throw "Unable to patch libewf build.ps1 vstools update block deterministically"
    }
    $PatchedBuildScriptContent = [regex]::Replace(
        $BuildScriptContent,
        $VsToolsUpdatePattern,
        'Else { Write-Host "Using pinned vstools: $${VSToolsPath}" }'
    )
    if ($PatchedBuildScriptContent -eq $BuildScriptContent) {
        throw "libewf build.ps1 vstools patch did not change the file"
    }
    if ([regex]::IsMatch($PatchedBuildScriptContent, $VsToolsUpdatePattern)) {
        throw "libewf build.ps1 still contains the unpinned vstools update block after patching"
    }
    Set-Content -Path $BuildScript -Value $PatchedBuildScriptContent -Encoding UTF8

    .\build.ps1 `
        -VisualStudioVersion 2022 `
        -Configuration Release `
        -Platform x64 `
        -PythonPath $PythonPath `
        -VSToolsPath $VsToolsDir `
        -VSToolsOptions "--extend-with-x64 --no-python-dll"
    if ($LASTEXITCODE -ne 0) {
        throw "libewf build.ps1 failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$LibEwfLib = Get-ChildItem -Path $SourceDir.FullName -Recurse -Filter "libewf.lib" |
    Where-Object { $_.FullName -match "\\Release\\" } |
    Select-Object -First 1
$LibEwfDll = Get-ChildItem -Path $SourceDir.FullName -Recurse -Filter "libewf.dll" |
    Where-Object { $_.FullName -match "\\Release\\" } |
    Select-Object -First 1

if (-not $LibEwfLib -or -not $LibEwfDll) {
    throw "Unable to locate libewf.lib and libewf.dll after build"
}

$RuntimeDir = $LibEwfDll.Directory.FullName
Copy-Item $LibEwfLib.FullName -Destination $LibDir -Force

$AllowedRuntimeDlls = @(
    "libewf.dll",
    "zlib.dll",
    "zlib1.dll",
    "bzip2.dll",
    "bz2.dll",
    "libbz2.dll",
    "libbfio.dll",
    "libcaes.dll",
    "libcdata.dll",
    "libcerror.dll",
    "libcfile.dll",
    "libclocale.dll",
    "libcnotify.dll",
    "libcpath.dll",
    "libcsplit.dll",
    "libcthreads.dll",
    "libfcache.dll",
    "libfdata.dll",
    "libfdatetime.dll",
    "libfguid.dll",
    "libfvalue.dll",
    "libhmac.dll",
    "libuna.dll"
)
$RuntimeDlls = Get-ChildItem -Path $RuntimeDir -Filter "*.dll"
$UnexpectedRuntimeDlls = $RuntimeDlls | Where-Object { $AllowedRuntimeDlls -notcontains $_.Name }
if ($UnexpectedRuntimeDlls) {
    $Names = ($UnexpectedRuntimeDlls | ForEach-Object { $_.Name }) -join ", "
    throw "Unexpected DLLs in libewf runtime directory: $Names"
}

foreach ($DllName in $AllowedRuntimeDlls) {
    $DllPath = Join-Path $RuntimeDir $DllName
    if (Test-Path $DllPath) {
        Copy-Item $DllPath -Destination $BinDir -Force
    }
}
if (-not (Test-Path (Join-Path $BinDir "libewf.dll"))) {
    throw "Missing required runtime DLL: libewf.dll"
}

$LibEwfLicenseDir = Join-Path $LicenseDir "libewf"
foreach ($Name in @("AUTHORS", "COPYING", "COPYING.LESSER", "NEWS", "README")) {
    Copy-RequiredFile -SourceDir $SourceDir.FullName -DestinationDir $LibEwfLicenseDir -Name $Name
}

$ZlibDir = Join-Path $SiblingRoot "zlib"
if (-not (Test-Path $ZlibDir)) {
    throw "Missing expected zlib source directory: $ZlibDir"
}
$ZlibLicenseDir = Join-Path $LicenseDir "zlib"
foreach ($Name in @("README", "zlib.h")) {
    Copy-RequiredFile -SourceDir $ZlibDir -DestinationDir $ZlibLicenseDir -Name $Name
}

if (-not (Test-Path $Bzip2Dir)) {
    throw "Missing expected bzip2 source directory: $Bzip2Dir"
}
$Bzip2LicenseDir = Join-Path $LicenseDir "bzip2"
foreach ($Name in @("LICENSE", "README")) {
    Copy-RequiredFile -SourceDir $Bzip2Dir -DestinationDir $Bzip2LicenseDir -Name $Name
}

Write-Host "libewf Windows build staged in $OutputPath"
