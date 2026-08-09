import { describe, expect, test } from "bun:test"
import { djinnPermissionPreviewLines, djinnPermissionResources } from "../../src/djinn/permission-approval"

describe("Djinn permission approval", () => {
  test("renders structured patch previews", () => {
    const lines = djinnPermissionPreviewLines({
      preview: [
        {
          operation: "update",
          relative_path: "src/lib.rs",
          lines_added: 1,
          lines_removed: 1,
          hunks: [
            {
              lines: [
                { kind: "context", content: "fn answer() -> i32 {" },
                { kind: "remove", content: "    41" },
                { kind: "add", content: "    42" },
                { kind: "context", content: "}" },
              ],
            },
          ],
        },
      ],
    })

    expect(lines.map((line) => line.text)).toContain("update src/lib.rs (+1/-1)")
    expect(lines.map((line) => line.text)).toContain("  @@ hunk 1")
    expect(lines.map((line) => line.text)).toContain("  -     41")
    expect(lines.map((line) => line.text)).toContain("  +     42")
  })

  test("collects unique file and resource scopes", () => {
    expect(
      djinnPermissionResources({
        preview: [{ path: "/tmp/a.txt", new_path: "/tmp/b.txt" }, { path: "/tmp/a.txt" }],
        resources: ["shell:printf hello"],
        resource: "shell:printf hello",
      }),
    ).toEqual(["/tmp/a.txt", "/tmp/b.txt", "shell:printf hello"])
  })

  test("falls back to metadata lines when no patch preview exists", () => {
    expect(djinnPermissionPreviewLines({ resource: "printf hello", workspace: "/tmp/work" }).map((line) => line.text)).toEqual([
      "resource: printf hello",
      "workspace: /tmp/work",
    ])
  })
})
