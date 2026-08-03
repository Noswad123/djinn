import type { Argv } from "yargs"
import path from "path"
import { cmd } from "./cmd"

type Release = {
  tag_name: string
  name?: string | null
  body?: string | null
  html_url?: string
  published_at?: string | null
  draft?: boolean
  prerelease?: boolean
}

export const UpstreamCommand = cmd({
  command: "upstream",
  describe: "inspect upstream opencode changes",
  builder: (yargs: Argv) => yargs.command(UpstreamChangelogCommand).demandCommand(),
  async handler() {},
})

const UpstreamChangelogCommand = cmd({
  command: "changelog [release]",
  describe: "download upstream opencode release notes",
  builder: (yargs: Argv) =>
    yargs
      .positional("release", {
        describe: "single upstream version to download, for example 1.18.10 or v1.18.10",
        type: "string",
      })
      .option("from", {
        describe: "oldest version to exclude when downloading a range",
        type: "string",
      })
      .option("to", {
        describe: "newest version to include when downloading a range",
        type: "string",
      })
      .option("limit", {
        alias: "n",
        describe: "maximum number of releases to download",
        type: "number",
        default: 10,
      })
      .option("output", {
        alias: "o",
        describe: "file to write",
        type: "string",
        default: "UPSTREAM_OPENCODE_CHANGELOG.md",
      })
      .option("print", {
        describe: "print the downloaded changelog after writing it",
        type: "boolean",
        default: false,
      }),
  async handler(args) {
    const releases = args.release ? [await fetchRelease(args.release)] : await fetchReleases()
    const selected = args.release ? releases : selectRange(releases, args)
    const output = formatChangelog(selected, args)
    await Bun.file(path.resolve(String(args.output))).write(output)
    console.log(`Downloaded ${selected.length} upstream opencode release note${selected.length === 1 ? "" : "s"} to ${args.output}`)
    if (args.print) process.stdout.write(output)
  },
})

async function fetchRelease(version: string) {
  const response = await fetch(`https://api.github.com/repos/anomalyco/opencode/releases/tags/${tag(version)}`, {
    headers: { "User-Agent": "buddy-upstream-changelog" },
  })
  if (!response.ok) throw new Error(`Failed to fetch upstream opencode ${tag(version)}: ${response.statusText}`)
  return requireRelease(await response.json())
}

async function fetchReleases() {
  const response = await fetch("https://api.github.com/repos/anomalyco/opencode/releases?per_page=100", {
    headers: { "User-Agent": "buddy-upstream-changelog" },
  })
  if (!response.ok) throw new Error(`Failed to fetch upstream opencode releases: ${response.statusText}`)
  const releases = await response.json()
  if (!Array.isArray(releases)) throw new Error("Unexpected upstream opencode releases response")
  return releases.map(requireRelease).filter((release) => !release.draft)
}

function selectRange(releases: Release[], args: { from?: string; to?: string; limit?: number }) {
  const start = args.to ? releases.findIndex((release) => sameVersion(release.tag_name, args.to!)) : 0
  const candidates = releases.slice(start === -1 ? 0 : start)
  const end = args.from ? candidates.findIndex((release) => sameVersion(release.tag_name, args.from!)) : -1
  return (end === -1 ? candidates : candidates.slice(0, end)).slice(0, Math.max(1, args.limit ?? 10))
}

function formatChangelog(releases: Release[], args: { from?: string; to?: string; release?: string }) {
  const heading = args.release
    ? `# Upstream opencode ${tag(args.release)}`
    : `# Upstream opencode changelog${args.from ? ` since ${tag(args.from)}` : ""}${args.to ? ` through ${tag(args.to)}` : ""}`
  return [
    heading,
    "",
    `Downloaded: ${new Date().toISOString()}`,
    `Source: https://github.com/anomalyco/opencode/releases`,
    "",
    ...releases.flatMap((release) => [
      `## ${release.tag_name}${release.name && release.name !== release.tag_name ? ` - ${release.name}` : ""}`,
      "",
      release.published_at ? `Published: ${release.published_at}` : undefined,
      release.html_url ? `URL: ${release.html_url}` : undefined,
      release.prerelease ? "Prerelease: yes" : undefined,
      "",
      release.body?.trim() || "_No release notes provided._",
      "",
    ]),
  ]
    .filter((line): line is string => line !== undefined)
    .join("\n")
}

function requireRelease(input: unknown): Release {
  if (typeof input !== "object" || input === null || !("tag_name" in input) || typeof input.tag_name !== "string") {
    throw new Error("Unexpected upstream opencode release response")
  }
  return {
    tag_name: input.tag_name,
    name: "name" in input && (typeof input.name === "string" || input.name === null) ? input.name : undefined,
    body: "body" in input && (typeof input.body === "string" || input.body === null) ? input.body : undefined,
    html_url: "html_url" in input && typeof input.html_url === "string" ? input.html_url : undefined,
    published_at:
      "published_at" in input && (typeof input.published_at === "string" || input.published_at === null)
        ? input.published_at
        : undefined,
    draft: "draft" in input && typeof input.draft === "boolean" ? input.draft : undefined,
    prerelease: "prerelease" in input && typeof input.prerelease === "boolean" ? input.prerelease : undefined,
  }
}

function sameVersion(left: string, right: string) {
  return normalize(left) === normalize(right)
}

function tag(version: string) {
  return version.startsWith("v") ? version : `v${version}`
}

function normalize(version: string) {
  return version.replace(/^v/, "")
}
