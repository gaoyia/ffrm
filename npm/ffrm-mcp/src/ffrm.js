import { spawn } from "node:child_process";
import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const packageRoot = join(here, "..");
export const releaseVersion = "0.1.4";
export const releaseUrl = `https://github.com/gaoyia/ffrm/releases/download/v${releaseVersion}/ffrm.exe`;

export function vendorPath() {
  return join(packageRoot, "vendor", "ffrm.exe");
}

export function ffrmPath() {
  const fromEnv = process.env.FFRM_BIN;
  if (fromEnv && existsSync(fromEnv)) {
    return fromEnv;
  }
  const vendor = vendorPath();
  if (existsSync(vendor)) {
    return vendor;
  }
  for (const name of ["debug", "release"]) {
    const built = join(packageRoot, "..", "..", "target", name, "ffrm.exe");
    if (existsSync(built)) {
      return built;
    }
  }
  return null;
}

export function unlockArgs(path, { force = false, confirm = false } = {}) {
  if (!confirm) {
    return null;
  }
  const args = ["unlock", path, "--yes"];
  if (force) {
    args.push("--force");
  }
  return args;
}

export function deleteArgs(path, { unlock = false, force = false, recursive = false, confirm = false } = {}) {
  if (!confirm) {
    return null;
  }
  if (force && !unlock) {
    return { error: "delete 使用 force 时必须同时把 unlock 设为 true。" };
  }
  const args = ["delete", path, "--yes"];
  if (unlock) {
    args.push("--unlock");
  }
  if (force) {
    args.push("--force");
  }
  if (recursive) {
    args.push("--recursive");
  }
  return args;
}

export function runFfrm(args) {
  const bin = ffrmPath();
  if (!bin) {
    return Promise.resolve({
      code: 1,
      stdout: "",
      stderr: "找不到 ffrm.exe。在 Windows 上重新安装 ffrm-mcp，或设置 FFRM_BIN。",
    });
  }
  return new Promise((resolve) => {
    const child = spawn(bin, args, { windowsHide: true });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      resolve({ code: 1, stdout, stderr: `${stderr}${error.message}` });
    });
    child.on("close", (code) => {
      resolve({ code: code ?? 1, stdout, stderr });
    });
  });
}

export async function downloadRelease() {
  const destination = vendorPath();
  await mkdir(dirname(destination), { recursive: true });
  const response = await fetch(releaseUrl, { redirect: "follow" });
  if (!response.ok) {
    throw new Error(`下载 ffrm.exe 失败: HTTP ${response.status}`);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length < 64 || bytes[0] !== 0x4d || bytes[1] !== 0x5a) {
    throw new Error("下载到的文件不是 Windows 可执行文件。");
  }
  await writeFile(destination, bytes);
  return destination;
}

export async function copyLocalBuild() {
  const built = ffrmPath();
  const destination = vendorPath();
  if (!built || built === destination) {
    return null;
  }
  await mkdir(dirname(destination), { recursive: true });
  await copyFile(built, destination);
  return destination;
}
