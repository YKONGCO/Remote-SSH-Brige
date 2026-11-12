cd /d D:\code\remotesshmcp
set "PATH=%CD%\.tools\cargo\bin;%CD%\.tools\w64devkit\bin;%PATH%"
set "CARGO_HOME=%CD%\.tools\cargo"
set "RUSTUP_HOME=%CD%\.tools\rustup"
set "OPENSSL_DIR=%CD%\.tools\openssl"
set "OPENSSL_STATIC=1"
set "OPENSSL_NO_VENDOR=1"
set "LIBRARY_PATH=%CD%\.tools\compiler-compat"
npm.cmd run desktop