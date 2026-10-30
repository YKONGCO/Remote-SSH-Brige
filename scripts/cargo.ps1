param([Parameter(ValueFromRemainingArguments=$true)][string[]]$CargoArgs)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$localCargo = Join-Path $projectRoot '.tools\cargo\bin'
if (Test-Path (Join-Path $localCargo 'cargo.exe')) {
    $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
    $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
    $env:PATH = "$localCargo;$(Join-Path $projectRoot '.tools\w64devkit\bin');$env:PATH"
    $env:RUSTFLAGS = '-C link-self-contained=yes'
    $localOpenSSL = Join-Path $projectRoot '.tools\openssl'
    if (Test-Path (Join-Path $localOpenSSL 'lib\libcrypto.a')) {
        $env:OPENSSL_DIR = $localOpenSSL
        $env:OPENSSL_NO_VENDOR = '1'
        $env:OPENSSL_STATIC = '1'
    }
}
& cargo @CargoArgs
exit $LASTEXITCODE
