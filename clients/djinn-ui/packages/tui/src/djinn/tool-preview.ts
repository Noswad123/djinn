export function toolPreview(preview: string) {
  return preview
    .split("\n")
    .filter((line) => {
      const trimmed = line.trimStart()
      return ![
        "# @name:",
        "# @description:",
        "# @url:",
        "// @name:",
        "// @description:",
        "// @url:",
        "-- @name:",
        "-- @description:",
        "-- @url:",
      ].some((prefix) => trimmed.startsWith(prefix))
    })
    .join("\n")
    .split("")
    .filter((ch) => ch === "\n" || ch === "\t" || (ch.charCodeAt(0) >= 32 && ch.charCodeAt(0) !== 127))
    .join("")
    .replaceAll("\t", " ")
}
