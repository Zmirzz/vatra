// Builds `vatra` and places it where Tauri expects a sidecar, named with the target triple.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const profile = process.argv[2] === "debug" ? "debug" : "release";

const cargoArgs = ["build", "-p", "vatra-cli"];
if (profile === "release") cargoArgs.push("--release");
execFileSync("cargo", cargoArgs, { cwd: root, stdio: "inherit" });

const triple = execFileSync("rustc", ["--print", "host-tuple"], { encoding: "utf8" }).trim();
const binaries = join(here, "..", "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
copyFileSync(join(root, "target", profile, "vatra.exe"), join(binaries, `vatra-${triple}.exe`));
