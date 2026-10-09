import { copyLocalBuild, downloadRelease } from "./ffrm.js";

if (process.platform !== "win32") {
  console.error("ffrm-mcp 只支持 Windows，已跳过下载。");
  process.exit(0);
}

try {
  const path = await downloadRelease();
  console.error(`已下载 ${path}`);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  const local = await copyLocalBuild().catch(() => null);
  if (local) {
    console.error(`${message} 已改用本地构建 ${local}`);
    process.exit(0);
  }
  console.error(message);
  process.exit(1);
}
