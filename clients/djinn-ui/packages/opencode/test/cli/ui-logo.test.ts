import { expect, test } from "bun:test"
import { UI } from "../../src/cli/ui"

test("plain CLI logo uses the Djinn wordmark", () => {
  const logo = UI.logo()

  expect(logo).toContain("█▀▀▄    █ █ █▄  █ █▄  █")
  expect(logo).not.toContain("█▀▀▄ █  █ █▀▀▄ █▀▀▄ █  █")
})
