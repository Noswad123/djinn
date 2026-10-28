import { expect, test } from "bun:test"
import { Schema } from "effect"
import { DjinnLiveObservation } from "@opencode-ai/schema/djinn-live-observation"
import { createRegistry, sample } from "../../src/session/live-observation"
import fixture from "../../../schema/test/djinn-live-observation.fixture.json"

test("producer matches the cross-language fixture apart from fresh UUID identities", () => {
  const expected = Schema.decodeUnknownSync(DjinnLiveObservation.Observation)(fixture)
  const registry = createRegistry()
  const instance = registry.registerInstance()
  instance.track("ses_cross_language_busy", { busy: true })
  instance.track("ses_cross_language_unknown", { busy: false })
  expect(sample(registry, fixture.sequence, fixture.sampled_at_ms)).toEqual({
    ...expected,
    runtime_id: registry.runtimeID,
    instances: [{ ...expected.instances[0], instance_id: instance.instanceID }],
  })
})

test("only actual owner busy getters provide positive evidence; snapshots are detached", () => {
  const registry = createRegistry()
  const instance = registry.registerInstance()
  const owner = { busy: false }
  const handle = instance.track("ses_opaque", owner)
  const before = registry.snapshot()
  expect(before[0].sessions).toEqual([{ session_id: "ses_opaque", activity: "unknown" }])
  owner.busy = true
  expect(registry.snapshot()[0].sessions[0].activity).toBe("busy")
  expect(before[0].sessions[0].activity).toBe("unknown")
  expect(registry.snapshot()).toEqual(registry.snapshot())
  handle.remove()
  owner.busy = false
  owner.busy = true
  expect(registry.snapshot()[0].sessions).toEqual([])
  const successor = instance.track("ses_opaque", { busy: true })
  handle.remove()
  expect(registry.snapshot()[0].sessions[0].activity).toBe("busy")
  successor.remove()
  expect(registry.snapshot()[0].sessions).toEqual([])
  instance.close()
  instance.track("ses_late", owner)
  expect(registry.snapshot()).toEqual([])
  expect(registry.registerInstance().instanceID).not.toBe(instance.instanceID)
  expect(createRegistry().runtimeID).not.toBe(registry.runtimeID)
})

test("caps are global and rejected instance and runner handles remain inert", () => {
  const registry = createRegistry()
  const instances = Array.from({ length: 17 }, () => registry.registerInstance())
  expect(registry.snapshot()).toHaveLength(16)
  const handles = Array.from({ length: 128 }, (_, i) => instances[i % 2].track(`ses_${i}`, { busy: true }))
  const rejected = instances[2].track("ses_overflow", { busy: true })
  expect(registry.snapshot().flatMap((x) => x.sessions)).toHaveLength(128)
  handles[0].remove()
  rejected.remove()
  expect(registry.snapshot().flatMap((x) => x.sessions)).toHaveLength(127)
  instances[0].close()
  instances[16].track("ses_rejected_instance", { busy: true })
  expect(registry.snapshot().some((x) => x.instance_id === instances[16].instanceID)).toBe(false)
  instances[2].track("ses_new", { busy: false })
  expect(
    registry
      .snapshot()
      .flatMap((x) => x.sessions)
      .some((x) => x.session_id === "ses_new"),
  ).toBe(true)
  for (const instance of instances) instance.close()
  for (const handle of handles) handle.remove()
  expect(registry.snapshot()).toEqual([])
  const fresh = registry.registerInstance()
  for (let i = 0; i < 128; i++) fresh.track(`ses_fresh_${i}`, { busy: true })
  expect(registry.snapshot()[0].sessions).toHaveLength(128)
})

test("invalid wire identities are ignored without tightening legacy IDs", () => {
  const registry = createRegistry()
  const instance = registry.registerInstance()
  for (const id of ["", "has space", "ses_\n", "ses_é", "a".repeat(129)]) instance.track(id, { busy: true })
  expect(registry.snapshot()[0].sessions).toEqual([])
  instance.track("!".repeat(128), { busy: true })
  expect(registry.snapshot()[0].sessions).toHaveLength(1)
})

test("wire contract rejects idle, unsafe counts, noncanonical UUIDs and duplicate identities", () => {
  const registry = createRegistry()
  registry.registerInstance().track("ses_wire", { busy: true })
  const value = sample(registry, 1, 1234)
  const decode = Schema.decodeUnknownSync(DjinnLiveObservation.Observation)
  expect(decode(value)).toEqual(value)
  for (const bad of [
    { ...value, sequence: 0 },
    { ...value, sequence: Number.MAX_SAFE_INTEGER + 1 },
    { ...value, sampled_at_ms: -1 },
    { ...value, valid_for_ms: 3001 },
    { ...value, runtime_id: value.runtime_id.toUpperCase() },
    { ...value, runtime_id: "00000000-0000-4000-7000-000000000000" },
    { ...value, instances: [...value.instances, ...value.instances] },
    { ...value, instances: [{ ...value.instances[0], sessions: [{ session_id: "ses_x", activity: "idle" }] }] },
    {
      ...value,
      instances: [
        { ...value.instances[0], sessions: [...value.instances[0].sessions, ...value.instances[0].sessions] },
      ],
    },
    {
      ...value,
      instances: [
        {
          ...value.instances[0],
          sessions: Array.from({ length: 129 }, (_, i) => ({ session_id: `ses_${i}`, activity: "busy" })),
        },
      ],
    },
  ])
    expect(() => decode(bad)).toThrow()
})
