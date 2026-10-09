import { McpServer } from "@modelcontextprotocol/server";
import * as z from "zod/v4";

import { deleteArgs, releaseVersion, runFfrm, unlockArgs } from "./ffrm.js";

function textResult(text, isError = false) {
  return { content: [{ type: "text", text }], isError };
}

function commandResult(result) {
  const text = `${result.stdout}${result.stderr}`.trim();
  return textResult(text || `ffrm 退出码 ${result.code}`, result.code !== 0);
}

async function requireWindows() {
  if (process.platform === "win32") {
    return null;
  }
  return textResult("ffrm 只支持 Windows。", true);
}

export function createServer() {
  const server = new McpServer({ name: "ffrm", version: releaseVersion });

  server.registerTool(
    "status",
    {
      description: "列出占用本地文件或文件夹的进程。List processes locking a local path. Windows only.",
      inputSchema: z.object({
        path: z.string().min(1).describe("要查看的文件或文件夹路径"),
      }),
    },
    async ({ path }) => {
      const unsupported = await requireWindows();
      if (unsupported) {
        return unsupported;
      }
      return commandResult(await runFfrm(["status", path]));
    },
  );

  server.registerTool(
    "unlock",
    {
      description:
        "解除占用。会让占用进程退出，没有退出时结束该进程。confirm 必须为 true 才会执行。Refuses critical system processes.",
      inputSchema: z.object({
        path: z.string().min(1).describe("要释放的文件或文件夹路径"),
        force: z.boolean().optional().describe("跳过等待，直接结束进程"),
        confirm: z.boolean().describe("必须为 true 才会解除占用"),
      }),
    },
    async ({ path, force, confirm }) => {
      const unsupported = await requireWindows();
      if (unsupported) {
        return unsupported;
      }
      const args = unlockArgs(path, { force, confirm });
      if (!args) {
        const status = await runFfrm(["status", path]);
        const seen = `${status.stdout}${status.stderr}`.trim();
        return textResult(`${seen}\n没有执行。把 confirm 设为 true 才会解除占用。`.trim(), true);
      }
      return commandResult(await runFfrm(args));
    },
  );

  server.registerTool(
    "delete",
    {
      description:
        "删除文件或文件夹。confirm 必须为 true 才会执行。目录里还有内容时把 recursive 设为 true。文件被占用时把 unlock 设为 true。",
      inputSchema: z.object({
        path: z.string().min(1).describe("要删除的文件或文件夹路径"),
        unlock: z.boolean().optional().describe("删除前先解除占用"),
        force: z.boolean().optional().describe("解除占用时跳过等待。必须同时把 unlock 设为 true"),
        recursive: z.boolean().optional().describe("删除非空目录"),
        confirm: z.boolean().describe("必须为 true 才会删除"),
      }),
    },
    async ({ path, unlock, force, recursive, confirm }) => {
      const unsupported = await requireWindows();
      if (unsupported) {
        return unsupported;
      }
      const args = deleteArgs(path, { unlock, force, recursive, confirm });
      if (!args) {
        const status = await runFfrm(["status", path]);
        const seen = `${status.stdout}${status.stderr}`.trim();
        return textResult(`${seen}\n没有执行。把 confirm 设为 true 才会删除。`.trim(), true);
      }
      if (args.error) {
        return textResult(args.error, true);
      }
      return commandResult(await runFfrm(args));
    },
  );

  return server;
}
