import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { deleteArgs, ffrmPath, runFfrm, unlockArgs } from "./ffrm.js";

test("unlock stays idle until confirm is true", () => {
  assert.equal(unlockArgs("D:\\temp\\a.txt", { confirm: false }), null);
  assert.deepEqual(unlockArgs("D:\\temp\\a.txt", { confirm: true }), ["unlock", "D:\\temp\\a.txt", "--yes"]);
  assert.deepEqual(unlockArgs("D:\\temp\\a.txt", { confirm: true, force: true }), [
    "unlock",
    "D:\\temp\\a.txt",
    "--yes",
    "--force",
  ]);
});

test("delete requires confirm, and force requires unlock", () => {
  assert.equal(deleteArgs("D:\\temp\\a.txt", { confirm: false, unlock: true }), null);
  assert.deepEqual(deleteArgs("D:\\temp\\a.txt", { confirm: true, force: true }), {
    error: "delete 使用 force 时必须同时把 unlock 设为 true。",
  });
  assert.deepEqual(deleteArgs("D:\\temp\\a.txt", { confirm: true, unlock: true, force: true, recursive: true }), [
    "delete",
    "D:\\temp\\a.txt",
    "--yes",
    "--unlock",
    "--force",
    "--recursive",
  ]);
});

test("status reads a free file through the local ffrm binary", async () => {
  const bin = ffrmPath();
  if (!bin) {
    return;
  }
  const dir = await mkdtemp(join(tmpdir(), "ffrm-mcp-"));
  const file = join(dir, "free.txt");
  await writeFile(file, "ok");
  const result = await runFfrm(["status", file]);
  await rm(dir, { recursive: true, force: true });
  assert.equal(result.code, 0);
  assert.match(result.stdout, /没有进程占用此文件/);
});
