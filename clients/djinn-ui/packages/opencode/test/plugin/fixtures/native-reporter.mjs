import { appendFileSync, readFileSync } from "node:fs"

// A bounded, local executable used only by native reporter transport tests.
const [output, mode, literal] = process.argv.slice(2)
const input = readFileSync(0, "utf8")
appendFileSync(output, JSON.stringify({ pid: process.pid, input: JSON.parse(input), literal }) + "\n")
if (mode === "hang") await new Promise(() => setTimeout(() => process.exit(0), 10000))
process.exit(mode === "fail" ? 1 : 0)
