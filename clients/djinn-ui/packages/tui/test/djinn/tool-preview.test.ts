import { describe, expect, test } from "bun:test"
import { toolPreview } from "../../src/djinn/tool-preview"

describe("Djinn tool preview", () => {
  for (const prefix of ["#", "//", "--"]) {
    test(`strips ${prefix} metadata including URL with leading whitespace`, () => {
      expect(
        toolPreview(
          [
            `${prefix} @name: example`,
            `  ${prefix} @description: An example tool`,
            `\t${prefix} @url: https://example.com/tool`,
            `${prefix} Keep this comment`,
            "run_example()",
          ].join("\n"),
        ),
      ).toBe(`${prefix} Keep this comment\nrun_example()`)
    })
  }

  test("preserves source containing URLs and non-metadata tags", () => {
    const preview = [
      "#!/bin/sh",
      "# @usage: example",
      "# @urlish: not metadata",
      'echo "# @url: https://example.com"',
      'fetch("https://example.com")',
      "",
    ].join("\n")
    expect(toolPreview(preview)).toBe(preview)
  })

  test("sanitizes control characters and tabs while preserving newlines", () => {
    expect(toolPreview("\u0000run\u0007\u007f\ttool\r\n\tnext")).toBe("run tool\n next")
  })

  test("handles empty and metadata-only previews", () => {
    expect(toolPreview("")).toBe("")
    expect(toolPreview("# @name: example\n// @description: Example\n-- @url: https://example.com")).toBe("")
  })
})
