import { expect } from "bun:test"
import { Effect, Layer } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import { SessionV1 } from "@opencode-ai/core/v1/session"
import { ProviderV2 } from "@opencode-ai/core/provider"
import { ModelV2 } from "@opencode-ai/core/model"
import { EventV2Bridge } from "@/event-v2-bridge"
import { Session } from "@/session/session"
import { SessionSummary } from "@/session/summary"
import { MessageID, PartID } from "@/session/schema"
import { Snapshot } from "@/snapshot"
import { testEffect } from "../lib/effect"

const it = testEffect(
  LayerNode.compile(LayerNode.group([SessionSummary.node, SessionProjector.node, Session.node, EventV2Bridge.node]), [
    [
      Snapshot.node,
      Layer.mock(Snapshot.Service, {
        diffFull: () =>
          Effect.succeed<Snapshot.FileDiff[]>([
            {
              file: "changed.txt",
              patch: "-before\n+after\n",
              additions: 1,
              deletions: 1,
              status: "modified",
            },
          ]),
      }),
    ],
  ]),
)

it.instance(
  "summary skips redundant durable writes but persists changed diffs",
  () =>
    Effect.gen(function* () {
      const sessions = yield* Session.Service
      const summary = yield* SessionSummary.Service
      const events = yield* EventV2Bridge.Service
      const session = yield* sessions.create({})
      const messageID = MessageID.ascending()
      yield* sessions.updateMessage({
        id: messageID,
        sessionID: session.id,
        role: "user",
        agent: "build",
        time: { created: Date.now() },
        model: { providerID: ProviderV2.ID.make("test"), modelID: ModelV2.ID.make("test") },
      })
      const writes: string[] = []
      const off = yield* events.listen((event) => {
        if (
          event.type === Session.Event.Updated.type ||
          event.type === Session.Event.Diff.type ||
          event.type === SessionV1.Event.MessageUpdated.type
        )
          writes.push(event.type)
        return Effect.void
      })
      yield* Effect.addFinalizer(() => off)
      const input = { sessionID: session.id, messageID }
      yield* summary.summarize(input)
      const initial = writes.length
      expect(initial).toBeGreaterThan(0)
      yield* summary.summarize(input)
      expect(writes.length).toBe(initial)
      yield* sessions.updatePart({
        type: "step-start",
        id: PartID.ascending(),
        sessionID: session.id,
        messageID,
        snapshot: "before",
      })
      yield* sessions.updatePart({
        type: "step-finish",
        id: PartID.ascending(),
        sessionID: session.id,
        messageID,
        snapshot: "after",
        reason: "stop",
        cost: 0,
        tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
      })
      yield* summary.summarize(input)
      expect(writes.length).toBe(initial + 1)
      yield* sessions.setSummary({
        sessionID: session.id,
        summary: { additions: 2, deletions: 1, files: 1 },
      })
      const changed = writes.length
      yield* summary.summarize(input)
      expect((yield* sessions.get(session.id)).summary).toMatchObject({ additions: 0, deletions: 0, files: 0 })
      expect(writes.length).toBe(changed + 2)
      yield* summary.summarize(input)
      expect(writes.length).toBe(changed + 2)
      expect((yield* summary.diff(input))[0]?.file).toBe("changed.txt")
      yield* summary.summarize(input)
      expect(writes.length).toBe(changed + 2)
    }),
  { config: { snapshot: true } },
)
