#!/usr/bin/env node
import { appendFileSync, readFileSync } from "node:fs"
import { join } from "node:path"

const args = process.argv.slice(2)
const root = args[args.indexOf("--state-root") + 1]
const mode = process.env.DJINN_ATTACHMENT_TEST_MODE
if (args[1] === "attachment-target") {
  if (mode === "overflow") process.stdout.write("x".repeat(17000))
  else if (mode !== "hang") process.stdout.write(process.env.DJINN_ATTACHMENT_TEST_TARGET ?? "{}")
} else {
  appendFileSync(join(root, "receipts"), JSON.stringify({ args, input: JSON.parse(readFileSync(0, "utf8")) }) + "\n")
}
if (mode === "hang") setInterval(() => {}, 1000)
else process.exit(mode === "fail" ? 1 : 0)
