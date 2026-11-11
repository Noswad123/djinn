#!/usr/bin/env bun

import { $ } from "bun"
import path from "path"
import { fileURLToPath } from "url"
import { createSolidTransformPlugin } from "@opentui/solid/bun-plugin"
import { mkdtemp, readdir, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { isDeepStrictEqual } from "node:util"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const dir = path.resolve(__dirname, "..")

process.chdir(dir)

const generated = await import("./generate.ts")

import { Script } from "@opencode-ai/script"
import pkg from "../package.json"

const skipInstall = process.argv.includes("--skip-install")
const outputFlag = process.argv.indexOf("--outdir")
if (outputFlag !== -1 && (!process.argv[outputFlag + 1] || process.argv[outputFlag + 1].startsWith("--"))) {
  throw new Error("--outdir requires a path")
}
const output = outputFlag === -1 ? path.join(dir, "dist") : path.resolve(process.argv[outputFlag + 1])
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

// An explicit output directory must be empty; never delete a caller-supplied path.
if (outputFlag === -1) await $`rm -rf ${output}`
if (outputFlag !== -1 && (await Bun.file(output).exists())) throw new Error("--outdir must not be a file")
if (outputFlag !== -1 && (await readdir(output).catch(() => [])).length) throw new Error("--outdir must be empty")

if (!skipInstall) {
  await $`bun install @opentui/core@${pkg.dependencies["@opentui/core"]}`
  await $`bun install @parcel/watcher@${pkg.dependencies["@parcel/watcher"]}`
  await $`bun install @ff-labs/fff-bun@${pkg.dependencies["@ff-labs/fff-bun"]}`
}

const binaryName = "djinn-ui"
const name = [binaryName, target.os, target.arch].join("-")
console.log(`building ${name}`)
await $`mkdir -p ${output}/${name}/bin`

const workerPath = "./src/cli/tui/worker.ts"
const treeSitterWorkerPath = "opentui-tree-sitter-worker.js"

const result = await Bun.build({
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
    target: ["bun", target.os, target.arch].join("-") as any,
    outfile: path.join(output, name, "bin", binaryName),
    execArgv: [`--user-agent=${binaryName}/${Script.version}`, "--use-system-ca", "--"],
    windows: {},
  },
  files: {
    [treeSitterWorkerPath]: treeSitterWorker,
    ...(embeddedFileMap ? { "opencode-web-ui.gen.ts": embeddedFileMap } : {}),
  },
  entrypoints: [
    "./src/index.ts",
    workerPath,
    treeSitterWorkerPath,
    ...(embeddedFileMap ? ["opencode-web-ui.gen.ts"] : []),
  ],
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
if (!result.success) throw new AggregateError(result.logs, `Build failed for ${name}`)

const binaryPath = path.join(output, name, "bin", binaryName)
console.log(`Validating isolated capabilities: ${binaryPath}`)
const isolated = await mkdtemp(path.join(tmpdir(), "djinn-ui-capabilities-"))
try {
  for (const compression of ["0", "1"]) {
    const probe = Bun.spawn([binaryPath, "capabilities", "--json"], {
      cwd: isolated,
      env: {
        HOME: isolated,
        XDG_CONFIG_HOME: path.join(isolated, "config"),
        XDG_DATA_HOME: path.join(isolated, "data"),
        XDG_CACHE_HOME: path.join(isolated, "cache"),
        XDG_STATE_HOME: path.join(isolated, "state"),
        OPENCODE_DB: path.join(isolated, "missing", "poison.db"),
        OPENCODE_CONFIG: path.join(isolated, "missing", "poison.json"),
        DJINN_EVENT_COMPRESSION: compression,
      },
      stdout: "pipe",
      stderr: "pipe",
      timeout: 10_000,
    })
    const [stdout, stderr, code] = await Promise.all([
      new Response(probe.stdout).text(),
      new Response(probe.stderr).text(),
      probe.exited,
    ])
    if (code !== 0 || stderr || stdout.length > 4096) throw new Error("Compiled capability probe failed")
    const actual = JSON.parse(stdout)
    if (
      actual.product !== "djinn-ui" ||
      actual.protocol !== "djinn-ui-capabilities" ||
      actual.version !== 1 ||
      actual.build !== Script.version ||
      !isDeepStrictEqual(actual.event_storage?.read, ["json", "djinn:event:v1:zlib"]) ||
      !isDeepStrictEqual(actual.event_storage?.write, ["json", "djinn:event:v1:zlib"]) ||
      actual.event_storage?.guard !== "djinn_event_storage_v1"
    )
      throw new Error("Compiled capability contract mismatch")
    if ((await readdir(isolated)).length) throw new Error("Capability probe created filesystem artifacts")
  }
  console.log("Compiled capability validation passed")
} finally {
  await rm(isolated, { recursive: true, force: true })
}

await $`rm -rf ${output}/${name}/bin/tui`
await Bun.file(path.join(output, name, "package.json")).write(
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
