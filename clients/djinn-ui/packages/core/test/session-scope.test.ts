import { expect, test } from "bun:test"
import { DateTime, Deferred, Effect, Fiber, Schema } from "effect"
import { eq } from "drizzle-orm"
import { Database } from "../src/database/database"
import { AppNodeBuilder } from "../src/effect/app-node-builder"
import { LayerNode } from "../src/effect/layer-node"
import { EventV2 } from "../src/event"
import { Project } from "../src/project"
import { ProjectTable } from "../src/project/sql"
import { AbsolutePath, RelativePath } from "../src/schema"
import { SessionEvent } from "../src/session/event"
import { SessionProjector } from "../src/session/projector"
import { SessionSchema } from "../src/session/schema"
import { SessionScope } from "../src/session/scope"
import { SessionTable } from "../src/session/sql"
import { SessionV1 } from "../src/v1/session"
import { testEffect } from "./lib/effect"

const it = testEffect(AppNodeBuilder.build(LayerNode.group([Database.node, EventV2.node, SessionProjector.node])))
const sessionID = SessionSchema.ID.make("ses_scope_projection")
const info: SessionV1.SessionInfo = {
  id: sessionID,
  slug: "scope",
  projectID: Project.ID.global,
  directory: "/synthetic",
  title: "synthetic scope",
  version: "test",
  time: { created: 0, updated: 0 },
}
const moved = {
  sessionID,
  location: { directory: AbsolutePath.make("/synthetic/destination") },
  subdirectory: RelativePath.make("destination"),
  timestamp: DateTime.makeUnsafe(1),
}

const seed = Effect.gen(function* () {
  const { db } = yield* Database.Service
  const events = yield* EventV2.Service
  yield* db
    .insert(ProjectTable)
    .values({ id: Project.ID.global, worktree: AbsolutePath.make("/synthetic"), sandboxes: [] })
    .run()
  yield* events.publish(SessionV1.Event.Created, { sessionID, info })
  return { db, events }
})

for (const kind of ["created", "updated", "moved", "deleted"] as const) {
  it.effect(`shared ${kind} projector invalidates scope without the legacy Session wrapper`, () =>
    Effect.gen(function* () {
      const { db, events } = yield* seed
      const id = kind === "created" ? SessionSchema.ID.make("ses_scope_created") : sessionID
      const before = SessionScope.capture(db.$client, id)
      if (kind === "created") yield* events.publish(SessionV1.Event.Created, { sessionID: id, info: { ...info, id } })
      if (kind === "updated")
        yield* events.publish(SessionV1.Event.Updated, {
          sessionID,
          info: {
            ...info,
            parentID: SessionSchema.ID.make("ses_scope_parent"),
            workspaceID: undefined,
            directory: "/different",
          },
        })
      if (kind === "moved") yield* events.publish(SessionEvent.Moved, moved)
      if (kind === "deleted") yield* events.publish(SessionV1.Event.Deleted, { sessionID, info })
      expect(before()).toBe(false)
      expect(SessionScope.capture(db.$client, id)()).toBe(true)
    }),
  )
}

it.effect("replayed move invalidates and an outer transaction rollback cannot release or resurrect a token early", () =>
  Effect.gen(function* () {
    const { db, events } = yield* seed
    const before = SessionScope.capture(db.$client, sessionID)
    let during: (() => boolean) | undefined
    const replay = {
      id: EventV2.ID.create(),
      type: EventV2.versionedType(SessionEvent.Moved.type, 1),
      seq: 1,
      aggregateID: sessionID,
      data: Schema.encodeSync(SessionEvent.Moved.data)(moved),
    }
    const exit = yield* db
      .transaction(() =>
        Effect.gen(function* () {
          yield* events.replay(replay)
          // The nested event transaction and projector have finished, but the outer commit has not.
          during = SessionScope.capture(db.$client, sessionID)
          expect(during()).toBe(false)
          return yield* Effect.fail("synthetic outer rollback")
        }),
      )
      .pipe(Effect.exit)
    expect(exit._tag).toBe("Failure")
    expect(before()).toBe(false)
    expect(during?.()).toBe(false)
    expect((yield* db.select().from(SessionTable).where(eq(SessionTable.id, sessionID)).get())?.directory).toBe(
      "/synthetic",
    )
    expect(SessionScope.capture(db.$client, sessionID)()).toBe(true)
    const fresh = SessionScope.capture(db.$client, sessionID)
    yield* events.replay(replay)
    expect(fresh()).toBe(false)
    expect((yield* db.select().from(SessionTable).where(eq(SessionTable.id, sessionID)).get())?.directory).toBe(
      "/synthetic/destination",
    )
  }),
)

it.effect("event transaction failure after projection releases its marker only after rollback", () =>
  Effect.gen(function* () {
    const { db, events } = yield* seed
    const before = SessionScope.capture(db.$client, sessionID)
    let during: (() => boolean) | undefined
    const exit = yield* events
      .publish(SessionEvent.Moved, moved, {
        commit: () =>
          Effect.gen(function* () {
            during = SessionScope.capture(db.$client, sessionID)
            expect(during()).toBe(false)
            return yield* Effect.die("synthetic post-projector failure")
          }),
      })
      .pipe(Effect.exit)
    expect(exit._tag).toBe("Failure")
    expect(before()).toBe(false)
    expect(during?.()).toBe(false)
    expect((yield* db.select().from(SessionTable).where(eq(SessionTable.id, sessionID)).get())?.directory).toBe(
      "/synthetic",
    )
    expect(SessionScope.capture(db.$client, sessionID)()).toBe(true)
  }),
)

test("scope markers are session/database local, bounded, and interruption-safe", async () => {
  const database = {}
  const otherDatabase = {}
  const a = SessionScope.capture(database, "a")
  const b = SessionScope.capture(database, "b")
  const separate = SessionScope.capture(otherDatabase, "a")
  await Effect.runPromise(
    SessionScope.mutate(
      database,
      "a",
      Effect.gen(function* () {
        expect(a()).toBe(false)
        expect(b()).toBe(true)
        expect(separate()).toBe(true)
        expect(SessionScope.capture(database, "a")()).toBe(false)
        return yield* Effect.interrupt
      }),
    ).pipe(Effect.exit),
  )
  expect(a()).toBe(false)
  expect(b()).toBe(true)
  expect(SessionScope.capture(database, "a")()).toBe(true)
  const fill = (i: number): Effect.Effect<void> =>
    i === 1025
      ? Effect.sync(() => {
          expect(SessionScope.capture(database, "untracked")()).toBe(false)
        })
      : SessionScope.mutate(
          database,
          "active-" + i,
          Effect.suspend(() => fill(i + 1)),
        )
  await Effect.runPromise(fill(0))
  expect(SessionScope.capture(database, "untracked")()).toBe(true)
  SessionScope.close(database)
  expect(SessionScope.capture(database, "untracked")()).toBe(false)
})

it.effect("transaction retention overflow is bounded and releases its fail-closed barrier", () =>
  Effect.gen(function* () {
    const database = {}
    yield* SessionScope.transaction(
      database,
      Effect.gen(function* () {
        for (let i = 0; i < 1100; i++) yield* SessionScope.mutate(database, String(i), Effect.void)
        expect(SessionScope.capture(database, "overflow")()).toBe(false)
      }),
    )
    expect(SessionScope.capture(database, "overflow")()).toBe(true)
  }),
)

it.effect("a child retaining an expired transaction context cannot leak a new mutation marker", () =>
  Effect.gen(function* () {
    const database = {}
    const start = yield* Deferred.make<void>()
    const child = yield* SessionScope.transaction(
      database,
      Deferred.await(start).pipe(
        Effect.andThen(SessionScope.mutate(database, "child", Effect.void)),
        Effect.forkScoped,
      ),
    )
    yield* Deferred.succeed(start, undefined)
    yield* Fiber.join(child)
    expect(SessionScope.capture(database, "child")()).toBe(true)
  }),
)
