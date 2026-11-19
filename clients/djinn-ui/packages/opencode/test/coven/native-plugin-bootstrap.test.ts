import { expect } from "bun:test"
import { Effect, Layer } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { Config } from "../../src/config/config"
import { RuntimeFlags } from "../../src/effect/runtime-flags"
import { InstanceRef } from "../../src/effect/instance-ref"
import { Plugin } from "../../src/plugin/index"
import { SessionStatus } from "../../src/session/status"
import { SessionID } from "../../src/session/schema"
import { withTmpdirInstance } from "../fixture/fixture"
import { awaitWithTimeout, testEffect } from "../lib/effect"

testEffect(
  LayerNode.compile(LayerNode.group([Plugin.node, SessionStatus.node, CrossSpawnSpawner.node]), [
    [Config.node, Layer.mock(Config.Service, { get: () => Effect.succeed({}) })],
    [RuntimeFlags.node, RuntimeFlags.layer({ disableDefaultPlugins: true, pure: true })],
  ]),
).instance("actual Plugin.init builds the session dependency graph and initializes separately per instance", () =>
  Effect.gen(function* () {
    const plugin = yield* Plugin.Service
    const statuses = yield* SessionStatus.Service
    const first = yield* InstanceRef
    const id = SessionID.make("ses_native_plugin_bootstrap")
    yield* awaitWithTimeout(plugin.init(), "first plugin bootstrap recursed or blocked", "5 seconds")
    const hooks = yield* plugin.list()
    expect(hooks).toEqual([])
    yield* statuses.set(id, { type: "busy" })
    yield* Effect.gen(function* () {
      expect((yield* InstanceRef)?.directory).not.toBe(first?.directory)
      yield* awaitWithTimeout(plugin.init(), "second plugin bootstrap recursed or blocked", "5 seconds")
      const second = yield* plugin.list()
      expect(second).toEqual([])
      expect(second).not.toBe(hooks)
      expect(yield* plugin.list()).toBe(second)
      expect(yield* statuses.get(id)).toEqual({ type: "idle" })
      yield* statuses.set(id, { type: "retry", attempt: 1, message: "retry", next: 1 })
    }).pipe(withTmpdirInstance())
    expect(yield* plugin.list()).toBe(hooks)
    expect(yield* statuses.get(id)).toEqual({ type: "busy" })
  }),
)
