#!/usr/bin/env bun

import { $ } from "bun"
import path from "path"
import { fileURLToPath } from "url"
import { createSolidTransformPlugin } from "@opentui/solid/bun-plugin"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const dir = path.resolve(__dirname, "..")

process.chdir(dir)

const generated = await import("./generate.ts")

import { Script } from "@opencode-ai/script"
import pkg from "../package.json"

const skipInstall = process.argv.includes("--skip-install")
const sourcemapsFlag = process.argv.includes("--sourcemaps")
const plugin = createSolidTransformPlugin()
const skipEmbedWebUi = process.argv.includes("--skip-embed-web-ui")

const createEmbeddedWebUIBundle = async () => {
  console.log(`Building Web UI to embed in the binary`)
  const appDir = path.join(import.meta.dirname, "../../app")
  const dist = path.join(appDir, "dist")
  await $`OPENCODE_CHANNEL=${Script.channel} bun run --cwd ${appDir} build`
  const files = (await Array.fromAsync(new Bun.Glob("**/*").scan({ cwd: dist })))
    .map((file) => file.replaceAll("\\", "/"))
    .filter((file) => !file.endsWith(".map"))
    .sort()
  const imports = files.map((file, i) => {
    const spec = path.relative(dir, path.join(dist, file)).replaceAll("\\", "/")
    return `import file_${i} from ${JSON.stringify(spec.startsWith(".") ? spec : `./${spec}`)} with { type: "file" };`
  })
  const entries = files.map((file, i) => `  ${JSON.stringify(file)}: file_${i},`)
  return [
    `// Import all files as file_$i with type: "file"`,
    ...imports,
    `// Export with original mappings`,
    `export default {`,
    ...entries,
    `}`,
  ].join("\n")
}

const embeddedFileMap = skipEmbedWebUi ? null : await createEmbeddedWebUIBundle()
const treeSitterWorker = await Bun.file(fileURLToPath(import.meta.resolve("@opentui/core/parser.worker"))).text()

const currentArch = (() => {
  if (process.arch === "arm64" || process.arch === "x64") return process.arch
  throw new Error(`unsupported arch: ${process.arch}`)
})()
const currentOs = (() => {
  if (process.platform === "darwin" || process.platform === "linux") return process.platform
  throw new Error(`unsupported OS: ${process.platform}`)
})()

const target = {
  os: currentOs,
  arch: currentArch,
}

await $`rm -rf dist`

if (!skipInstall) {
  await $`bun install @opentui/core@${pkg.dependencies["@opentui/core"]}`
  await $`bun install @parcel/watcher@${pkg.dependencies["@parcel/watcher"]}`
  await $`bun install @ff-labs/fff-bun@${pkg.dependencies["@ff-labs/fff-bun"]}`
}

const name = [pkg.name, target.os, target.arch].join("-")
console.log(`building ${name}`)
await $`mkdir -p dist/${name}/bin`

const workerPath = "./src/cli/tui/worker.ts"
const treeSitterWorkerPath = "opentui-tree-sitter-worker.js"

await Bun.build({
  conditions: ["bun", "node"],
  tsconfig: "./tsconfig.json",
  plugins: [plugin],
  external: ["node-gyp"],
  format: "esm",
  minify: true,
  sourcemap: sourcemapsFlag ? "linked" : "none",
  splitting: true,
  compile: {
    autoloadBunfig: false,
    autoloadDotenv: false,
    autoloadTsconfig: true,
    autoloadPackageJson: true,
    target: name.replace(pkg.name, "bun") as any,
    outfile: `dist/${name}/bin/buddy`,
    execArgv: [`--user-agent=buddy/${Script.version}`, "--use-system-ca", "--"],
    windows: {},
  },
  files: {
    [treeSitterWorkerPath]: treeSitterWorker,
    ...(embeddedFileMap ? { "opencode-web-ui.gen.ts": embeddedFileMap } : {}),
  },
  entrypoints: ["./src/index.ts", workerPath, treeSitterWorkerPath, ...(embeddedFileMap ? ["opencode-web-ui.gen.ts"] : [])],
  define: {
    FFF_LIBC: JSON.stringify("gnu"),
    OPENCODE_VERSION: `'${Script.version}'`,
    OPENCODE_MODELS_DEV: generated.modelsData,
    OTUI_TREE_SITTER_WORKER_PATH: `/$bunfs/root/${treeSitterWorkerPath}`,
    OPENCODE_WORKER_PATH: workerPath,
    OPENCODE_CHANNEL: `'${Script.channel}'`,
    OPENCODE_LIBC: target.os === "linux" ? "'glibc'" : "",
    ...(target.os === "linux" ? { "process.env.OPENTUI_LIBC": JSON.stringify("glibc") } : {}),
  },
})

const binaryPath = `dist/${name}/bin/buddy`
console.log(`Running smoke test: ${binaryPath} --version`)
try {
  const versionOutput = await $`${binaryPath} --version`.text()
  console.log(`Smoke test passed: ${versionOutput.trim()}`)
} catch (e) {
  console.error(`Smoke test failed for ${name}:`, e)
  process.exit(1)
}

await $`rm -rf ./dist/${name}/bin/tui`
await Bun.file(`dist/${name}/package.json`).write(
  JSON.stringify(
    {
      name,
      version: Script.version,
      preferUnplugged: true,
      os: [target.os],
      cpu: [target.arch],
    },
    null,
    2,
  ),
)
