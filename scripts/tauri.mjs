import { existsSync } from "node:fs";
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const env = { ...process.env };
const localCargo = join(projectRoot, ".tools", "cargo");
const cargoBin = join(localCargo, "bin");
if (process.platform === "win32" && existsSync(join(cargoBin, "cargo.exe"))) {
  env.CARGO_HOME = localCargo;
  env.RUSTUP_HOME = join(projectRoot, ".tools", "rustup");
  // Keep toolchain changes within this command and its child processes.
  const pathKey =
    Object.keys(env).find((key) => key.toLowerCase() === "path") || "PATH";
  env[pathKey] = [
    cargoBin,
    join(projectRoot, ".tools", "w64devkit", "bin"),
    env[pathKey] || "",
  ].join(delimiter);
  env.RUSTFLAGS = [env.RUSTFLAGS, "-C link-self-contained=yes"]
    .filter(Boolean)
    .join(" ");
  const openssl = join(projectRoot, ".tools", "openssl");
  if (existsSync(join(openssl, "lib", "libcrypto.a"))) {
    env.OPENSSL_DIR = openssl;
    env.OPENSSL_NO_VENDOR = "1";
    env.OPENSSL_STATIC = "1";
  }
}
const require = createRequire(import.meta.url);
const cli = join(
  dirname(require.resolve("@tauri-apps/cli/package.json")),
  "tauri.js",
);
const child = spawn(process.execPath, [cli, ...process.argv.slice(2)], {
  cwd: projectRoot,
  env,
  stdio: "inherit",
  windowsHide: true,
});
child.on("error", (error) => {
  console.error(error.message);
  process.exitCode = 1;
});
child.on("exit", (code) => {
  process.exitCode = code ?? 1;
});
