import { appendFileSync, existsSync, readFileSync } from "node:fs"
import path from "node:path"

const directory = process.argv[2]
const input = JSON.parse(readFileSync(0, "utf8"))
appendFileSync(path.join(directory, "receipts"), JSON.stringify({ input }) + "\n")
if (input.session_id === "blocker") {
  while (!existsSync(path.join(directory, "release"))) {
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
