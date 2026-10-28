import { expect, test } from "bun:test"
import { ModelV2 } from "@opencode-ai/core/model"
import { ProviderV2 } from "@opencode-ai/core/provider"
import { configuration } from "../../src/session/prompt-control-runtime"

const selected = {
  agent: "restricted-reviewer",
  model: { providerID: ProviderV2.ID.make("selected-provider"), id: ModelV2.ID.make("selected-model"), variant: "careful" },
}

test("native prompt preserves recorded agent, model and variant", () => {
  expect(configuration(selected)).toEqual({ agent: selected.agent,
    model: { providerID: selected.model.providerID, modelID: selected.model.id }, variant: "careful" })
})

test("missing native configuration never selects defaults", () => {
  expect(configuration({})).toBeUndefined()
  expect(configuration({ agent: selected.agent })).toBeUndefined()
  expect(configuration({ model: selected.model })).toBeUndefined()
})

test("missing variant means explicit default, not agent variant fallback", () => {
  expect(configuration({ agent: selected.agent, model: { ...selected.model, variant: undefined } })?.variant).toBe("default")
})
