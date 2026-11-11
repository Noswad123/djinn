import { access, chmod, copyFile, lstat, mkdir, mkdtemp, readdir, readlink, realpath, rename, rm, symlink } from "node:fs/promises"
import { constants } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { isDeepStrictEqual } from "node:util"

export function hostTarget(platform = process.platform, arch = process.arch, glibc = process.report?.getReport().header.glibcVersionRuntime) {
  if (!["darwin", "linux"].includes(platform) || !["arm64", "x64"].includes(arch))
    throw new Error(`Unsupported host: ${platform}/${arch}`)
  if (platform === "linux" && !glibc) throw new Error("Linux UI requires glibc; musl is not supported")
  return `djinn-ui-${platform}-${arch}`
}

export async function probe(binary: string, timeout = 10_000) {
  const isolated = await realpath(await mkdtemp(path.join(tmpdir(), "djinn-install-probe-")))
  try {
    const child = Bun.spawn([path.resolve(binary), "capabilities", "--json"], {
      cwd: isolated,
      env: {
        HOME: isolated,
        TMPDIR: isolated,
        XDG_CONFIG_HOME: path.join(isolated, "config"),
        XDG_DATA_HOME: path.join(isolated, "data"),
        XDG_CACHE_HOME: path.join(isolated, "cache"),
        XDG_STATE_HOME: path.join(isolated, "state"),
        OPENCODE_DB: path.join(isolated, "missing", "poison.db"),
        OPENCODE_CONFIG: path.join(isolated, "missing", "poison.json"),
      },
      stdout: "pipe", stderr: "pipe", timeout, killSignal: "SIGKILL",
    })
    const readers = [child.stdout.getReader(), child.stderr.getReader()]
    const read = async (reader: ReadableStreamDefaultReader<Uint8Array>) => {
      const chunks: Uint8Array[] = []
      let size = 0
      while (true) {
        const { value: chunk, done } = await reader.read()
        if (done) break
        size += chunk.length
        if (size > 4096) { child.kill("SIGKILL"); throw new Error("Capability output exceeds 4096 bytes") }
        chunks.push(chunk)
      }
      return Buffer.concat(chunks).toString("utf8")
    }
    const deadline = Promise.withResolvers<never>()
    const timer = setTimeout(() => deadline.reject(new Error("Capability probe total deadline exceeded")), timeout)
    const [stdout, stderr, code] = await Promise.race([
      Promise.all([read(readers[0]), read(readers[1]), child.exited]),
      deadline.promise,
    ]).finally(() => {
      clearTimeout(timer)
      if (child.exitCode === null) child.kill("SIGKILL")
      // Do not wait for EOF: descendants can retain the pipe after the child exits.
      for (const reader of readers) void reader.cancel().catch(() => {})
    })
    if (code !== 0 || stderr) throw new Error("Capability probe failed or timed out")
    const value = JSON.parse(stdout)
    if (value?.product !== "djinn-ui" || value.protocol !== "djinn-ui-capabilities" || value.version !== 1 ||
        typeof value.build !== "string" || !value.build || value.event_storage?.guard !== "djinn_event_storage_v1" ||
        !isDeepStrictEqual(value.event_storage.read, ["json", "djinn:event:v1:zlib"]) ||
        !isDeepStrictEqual(value.event_storage.write, ["json", "djinn:event:v1:zlib"]))
      throw new Error("Incompatible Djinn UI capability contract")
    if ((await readdir(isolated)).length) throw new Error("Capability probe created filesystem artifacts")
    return value
  } finally {
    await rm(isolated, { recursive: true, force: true })
  }
}

async function bundleDigest(folder: string) {
  const hash = new Bun.CryptoHasher("sha256")
  for (const name of ["djinn", "djinn-ui"]) {
    const file = Bun.file(path.join(folder, name))
    hash.update(`${name}:${file.size}:`)
    for await (const chunk of file.stream()) hash.update(chunk)
  }
  return hash.digest("hex")
}

export async function install(uiRoot: string, destination: string, cli?: string) {
  const candidate = path.resolve(uiRoot, hostTarget(), "bin/djinn-ui")
  await access(candidate, constants.X_OK)
  // Everything is staged on the destination filesystem before any public path changes.
  const directory = path.resolve(destination)
  await mkdir(directory, { recursive: true })
  if (!cli) {
    const publicCLI = await readlink(path.join(directory, "djinn")).catch((error) => {
      if (!["ENOENT", "EINVAL"].includes(error.code)) throw error
    })
    const current = await lstat(path.join(directory, ".djinn/current")).catch((error) => {
      if (error.code !== "ENOENT") throw error
    })
    if (publicCLI?.startsWith(".djinn/") || current) {
      const id = current?.isSymbolicLink() ? await readlink(path.join(directory, ".djinn/current")) : ""
      if (publicCLI !== ".djinn/current/djinn" || !/^[a-f0-9]{64}$/.test(id) ||
          await readlink(path.join(directory, "djinn-ui")).catch(() => undefined) !== ".djinn/current/djinn-ui")
        throw new Error("Invalid managed bundle; refusing standalone UI replacement")
      const bundle = path.join(directory, ".djinn", id)
      if (await realpath(bundle) !== path.join(await realpath(path.join(directory, ".djinn")), id) ||
          await bundleDigest(bundle) !== id)
        throw new Error("Managed bundle is damaged")
      cli = path.join(bundle, "djinn")
    }
  }
  if (cli) await access(cli, constants.X_OK)
  const stage = await mkdtemp(path.join(directory, ".djinn-stage-"))
  try {
    await copyFile(candidate, path.join(stage, "djinn-ui"))
    await chmod(path.join(stage, "djinn-ui"), 0o755)
    if (cli) { await copyFile(cli, path.join(stage, "djinn")); await chmod(path.join(stage, "djinn"), 0o755) }
    await probe(path.join(stage, "djinn-ui"))
    if (!cli) {
      await rename(path.join(stage, "djinn-ui"), path.join(directory, "djinn-ui"))
      return
    }
    const id = await bundleDigest(stage)
    const store = path.join(directory, ".djinn")
    await mkdir(store, { recursive: true })
    const bundle = path.join(store, id)
    if (await Bun.file(path.join(bundle, "djinn-ui")).exists()) {
      // Reuse only byte-identical payloads, and revalidate the installed executable.
      if (await bundleDigest(bundle) !== id) throw new Error("Existing bundle is damaged")
      await probe(path.join(bundle, "djinn-ui"))
    } else await rename(stage, bundle)
    for (const name of ["djinn", "djinn-ui", ".djinn/current"]) {
      const existing = await lstat(path.join(directory, name)).catch((error) => { if (error.code !== "ENOENT") throw error })
      if (existing?.isDirectory()) throw new Error(`Refusing to replace directory: ${name}`)
    }
    // Relative links keep the installation relocatable. Publish CLI last on migration.
    const links = await mkdtemp(path.join(directory, ".djinn-links-"))
    try {
      const entries = [[".djinn/current", id], ["djinn-ui", ".djinn/current/djinn-ui"], ["djinn", ".djinn/current/djinn"]]
      const pending = []
      for (const [name, target] of entries) {
        if (await readlink(path.join(directory, name)).catch(() => undefined) === target) continue
        const temporary = path.join(links, path.basename(name))
        await symlink(target, temporary)
        pending.push([temporary, path.join(directory, name)])
      }
      // On subsequent installs only current changes: one atomic rename switches both payloads.
      for (const [temporary, publicPath] of pending) await rename(temporary, publicPath)
    } finally { await rm(links, { recursive: true, force: true }) }
  } finally { await rm(stage, { recursive: true, force: true }) }
}

if (import.meta.main) {
  const [mode, first, second, third] = process.argv.slice(2)
  if (mode === "build-ui" && first && second) {
    const output = path.resolve(second)
    await mkdir(path.dirname(output), { recursive: true })
    const stage = await mkdtemp(path.join(path.dirname(output), ".ui-build-"))
    try {
      const child = Bun.spawn([process.execPath, "run", "build", "--skip-install", "--skip-embed-web-ui", "--outdir", stage], {
        cwd: path.resolve(first), stdout: "inherit", stderr: "inherit",
      })
      if (await child.exited !== 0) throw new Error("UI build failed")
      await probe(path.join(stage, hostTarget(), "bin/djinn-ui"))
      await rm(output, { recursive: true, force: true })
      await rename(stage, output)
    } finally { await rm(stage, { recursive: true, force: true }) }
  } else if (mode === "bundle" && first && second && third) await install(second, third, first)
  else if (mode === "ui" && first && second) await install(first, second)
  else throw new Error("Usage: install.ts bundle CLI UI_OUTPUT INSTALL_DIR | ui UI_OUTPUT INSTALL_DIR | build-ui PACKAGE OUTPUT")
}
