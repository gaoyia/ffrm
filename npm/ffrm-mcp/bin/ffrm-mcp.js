#!/usr/bin/env node
import { serveStdio } from "@modelcontextprotocol/server/stdio";

import { createServer } from "../src/server.js";

if (process.platform !== "win32") {
  console.error("ffrm-mcp 只支持 Windows。");
  process.exit(1);
}

void serveStdio(createServer);
