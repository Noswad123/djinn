import { expect, spyOn, test } from "bun:test"
import { randomUUID } from "node:crypto"
import { mkdir, mkdtemp, rm } from "node:fs/promises"
import { connect } from "node:net"
import path from "node:path"
import { Effect, Fiber, Layer } from "effect"
import { MoveSession } from "@opencode-ai/core/control-plane/move-session"
import { Database } from "@opencode-ai/core/database/database"
import { AppNodeBuilder } from "@opencode-ai/core/effect/app-node-builder"
import { EventV2 } from "@opencode-ai/core/event"
import { AbsolutePath } from "@opencode-ai/core/schema"
import { SessionSchema } from "@opencode-ai/core/session/schema"
import { PermissionV1 } from "@opencode-ai/core/v1/permission"
import { WorkspaceV2 } from "@opencode-ai/core/workspace"
import { AppRuntime } from "../../src/effect/app-runtime"
import { InstanceRef } from "../../src/effect/instance-ref"
import { EventV2Bridge } from "../../src/event-v2-bridge"
import { InstanceStore } from "../../src/project/instance-store"
import { Permission } from "../../src/permission"
import { Question } from "../../src/question"
import { Session } from "../../src/session/session"
import { SessionID } from "../../src/session/schema"
import { DecisionControl } from "../../src/session/decision-control"
import { PromptControlRuntime } from "../../src/session/prompt-control-runtime"
import { PromptControl } from "../../src/session/prompt-control"
import { WebSharing } from "../../src/session/web-sharing"
import { tmpdir } from "../fixture/fixture"

const workspaceID = WorkspaceV2.ID.make("wrk_scope_fence")

async function fixture() {
  const tmp = await tmpdir()
  const waits: Fiber.Fiber<unknown, unknown>[] = []
  const close = async () => {
    for (const wait of waits) await AppRuntime.runPromise(Fiber.interrupt(wait))
    await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.disposeDirectory(tmp.path)))
    await tmp[Symbol.asyncDispose]()
  }
  try {
    const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
    const run = <A, E>(
      effect: Effect.Effect<A, E, Session.Service | Question.Service | Permission.Service | EventV2Bridge.Service>,
    ) => AppRuntime.runPromise(effect.pipe(Effect.provideService(InstanceRef, ctx)))
    const sessions = await run(Session.Service)
    const questions = await run(Question.Service)
    const permissions = await run(Permission.Service)
    const session = await run(sessions.create({}))
    const database = await AppRuntime.runPromise(Database.Service)
    const events = await run(EventV2Bridge.Service)
    const move = async () => {
      const directory = path.join(tmp.path, "destination")
      await mkdir(directory)
      // Bind the actual native move operation to the same database and projector as legacy waits.
      await Effect.runPromise(
        MoveSession.Service.use((service) =>
          service.moveSession({
            sessionID: SessionSchema.ID.make(session.id),
            destination: { directory: AbsolutePath.make(directory) },
            moveChanges: false,
          }),
        ).pipe(
          Effect.provide(
            AppNodeBuilder.build(MoveSession.node, [
              [Database.node, Layer.succeed(Database.Service, database)],
              [EventV2.node, Layer.succeed(EventV2.Service, events)],
            ]),
          ),
        ),
      )
      expect((await run(sessions.get(session.id))).directory).toBe(directory)
    }
    waits.push(
      AppRuntime.runFork(
        questions
          .ask({
            sessionID: session.id,
            questions: [
              {
                question: "Synthetic scope choice",
                header: "Scope",
                custom: false,
                options: [{ label: "A", description: "First" }],
              },
            ],
          })
          .pipe(Effect.provideService(InstanceRef, ctx)),
      ),
    )
    waits.push(
      AppRuntime.runFork(
        permissions
          .ask({
            id: PermissionV1.ID.make("per_scope"),
            sessionID: session.id,
            permission: "synthetic",
            patterns: ["fixture-only"],
            always: [],
            metadata: {},
            ruleset: [],
          })
          .pipe(Effect.provideService(InstanceRef, ctx)),
      ),
    )
    const refs = await run(
      Effect.gen(function* () {
        for (;;) {
          const q = (yield* questions.attention(session.id)).value.requests[0]
          const p = (yield* permissions.attention(session.id)).value.requests[0]
          if (q?.details_sha256 && p?.details_sha256)
            return [q, p].map(
              (meta): DecisionControl.Reference => ({
                kind: meta.kind,
                request_id: meta.request_id,
                occurrence_id: meta.occurrence_id,
                details_sha256: meta.details_sha256!,
              }),
            )
          yield* Effect.yieldNow
        }
      }).pipe(Effect.timeout("2 seconds")),
    )
    const native = PromptControlRuntime.automaticCallbacks(tmp.path, true)
    const sharing = WebSharing.create({ directory: tmp.path, mode: "read_write", qualify: native.qualify })
    await sharing.select({
      frontend_id: randomUUID(),
      revision: 1,
      selection_generation: 1,
      selection: { sessionID: session.id, directory: tmp.path },
    })
    const grant = (await sharing.sample())!
    return {
      tmp,
      ctx,
      run,
      sessions,
      questions,
      permissions,
      session,
      native,
      sharing,
      grant,
      refs,
      move,
      async [Symbol.asyncDispose]() {
        sharing.close()
        try {
          await run(sessions.remove(session.id).pipe(Effect.ignore))
        } finally {
          await close()
        }
      },
    }
  } catch (error) {
    await close()
    throw error
  }
}

for (const change of ["workspace", "delete", "move"] as const) {
  test(`actual ${change} invalidates captured attention/detail and both final service claims without disposing context or waits`, async () => {
    await using f = await fixture()
    const attention = (await f.native.decisions.attention(f.session.id))!
    const details = await Promise.all(f.refs.map((ref) => f.native.decisions.detail(f.session.id, ref)))
    expect(attention.current()).toBe(true)
    expect(details.every((sample) => sample?.current())).toBe(true)
    if (change === "workspace") await f.run(f.sessions.setWorkspace({ sessionID: f.session.id, workspaceID }))
    else if (change === "move") await f.move()
    else await f.run(f.sessions.remove(f.session.id))
    expect(attention.current()).toBe(false)
    expect(details.every((sample) => sample?.current() === false)).toBe(true)
    expect(f.sharing.current(f.grant.grant_id, f.session.id, f.ctx)).toBe(true)
    expect(await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.peek(f.tmp.path)))).toBe(f.ctx)
    let claims = 0
    for (const [i, ref] of f.refs.entries()) {
      const service = ref.kind === "question" ? f.questions : f.permissions
      expect(
        await f.run(
          service.resolve({
            ...ref,
            session_id: f.session.id,
            action:
              ref.kind === "question" ? { type: "question_answer", answers: [["A"]] } : { type: "permission_once" },
            current: () => details[i]!.current() && f.sharing.current(f.grant.grant_id, f.session.id, f.ctx),
            onClaim: () => {
              claims++
            },
          }),
        ),
      ).toBe("decision_stale")
    }
    expect(claims).toBe(0)
    expect(await f.run(f.questions.list())).toHaveLength(1)
    expect(await f.run(f.permissions.list())).toHaveLength(1)
    expect(await f.native.decisions.attention(f.session.id)).toBeUndefined()
    if (change === "workspace") {
      await f.run(f.sessions.setWorkspace({ sessionID: f.session.id, workspaceID: undefined }))
      expect(attention.current()).toBe(false)
      expect(details.every((sample) => sample?.current() === false)).toBe(true)
      expect((await f.native.decisions.attention(f.session.id))?.current()).toBe(true)
    }
  })
}

test("scope tokens cannot be captured during an in-progress actual workspace publication", async () => {
  await using f = await fixture()
  const entered = Promise.withResolvers<void>()
  const release = Promise.withResolvers<void>()
  const before = await f.run(f.sessions.scopeFence(f.session.id))
  const off = await f.run(
    EventV2Bridge.Service.use((events) =>
      events.listen((event) => {
        if (
          event.type !== Session.Event.Updated.type ||
          (event.data as { sessionID: SessionID }).sessionID !== f.session.id
        )
          return Effect.void
        return Effect.promise(async () => {
          entered.resolve()
          await release.promise
        })
      }),
    ),
  )
  const mutation = f.run(f.sessions.setWorkspace({ sessionID: f.session.id, workspaceID }))
  try {
    await entered.promise
    expect(before()).toBe(false)
    const during = await f.run(f.sessions.scopeFence(f.session.id))
    expect(during()).toBe(false)
    expect(await f.native.decisions.attention(f.session.id)).toBeUndefined()
    release.resolve()
    await mutation
    expect(during()).toBe(false)
    expect(before()).toBe(false)
    expect((await f.run(f.sessions.scopeFence(f.session.id)))()).toBe(true)
  } finally {
    release.resolve()
    try {
      await mutation
    } finally {
      await f.run(off)
    }
  }
})

test("scope token is captured before the asynchronous eligibility read, not stamped onto stale session info", async () => {
  await using f = await fixture()
  const entered = Promise.withResolvers<void>()
  const release = Promise.withResolvers<void>()
  const get = f.sessions.get
  // Delay only the return of the real metadata read; the mutation uses the normal service/projector.
  const held = spyOn(f.sessions, "get").mockImplementation((id) =>
    get(id).pipe(
      Effect.flatMap((info) =>
        Effect.promise(async () => {
          entered.resolve()
          await release.promise
          return info
        }),
      ),
    ),
  )
  const reading = f.native.decisions.attention(f.session.id)
  try {
    await entered.promise
    await f.run(f.sessions.setWorkspace({ sessionID: f.session.id, workspaceID }))
    release.resolve()
    const sample = await reading
    expect(sample?.current() ?? false).toBe(false)
  } finally {
    release.resolve()
    try {
      await reading
    } finally {
      held.mockRestore()
    }
  }
})

for (const kind of ["question", "permission"] as const) {
  for (const change of ["workspace", "delete", "move"] as const) {
    test(`actual automatic ${kind} callback refuses ${change} after qualification but before service.resolve`, async () => {
      await using f = await fixture()
      const entered = Promise.withResolvers<void>()
      const release = Promise.withResolvers<void>()
      const service = kind === "question" ? f.questions : f.permissions
      const resolve = service.resolve
      // Scheduling gate only: execution always goes through the actual native conditional settlement.
      const held = spyOn(service, "resolve").mockImplementation((input) =>
        Effect.promise(async () => {
          entered.resolve()
          await release.promise
        }).pipe(Effect.andThen(resolve(input))),
      )
      let claims = 0
      const result = f.native.decisions.decide({
        ...f.refs.find((r) => r.kind === kind)!,
        session_id: f.session.id,
        action: kind === "question" ? { type: "question_answer", answers: [["A"]] } : { type: "permission_once" },
        current: (context) => f.sharing.current(f.grant.grant_id, f.session.id, context),
        onClaim: () => {
          claims++
        },
      })
      try {
        await entered.promise
        if (change === "workspace") await f.run(f.sessions.setWorkspace({ sessionID: f.session.id, workspaceID }))
        else if (change === "move") await f.move()
        else await f.run(f.sessions.remove(f.session.id))
        release.resolve()
        expect(await result).toBe("decision_stale")
        expect(claims).toBe(0)
        expect(await f.run(f.questions.list())).toHaveLength(1)
        expect(await f.run(f.permissions.list())).toHaveLength(1)
      } finally {
        release.resolve()
        try {
          await result
        } finally {
          held.mockRestore()
        }
      }
    })
  }
}

test("failed native mutation releases the in-progress scope fence without allocating session history", async () => {
  await using f = await fixture()
  const failed = await f.run(
    f.sessions.setWorkspace({ sessionID: SessionID.make("ses_scope_missing"), workspaceID }).pipe(Effect.exit),
  )
  expect(failed._tag).toBe("Failure")
  expect((await f.native.decisions.attention(f.session.id))?.current()).toBe(true)
})

test("observed scope tokens are bounded and capacity eviction fails closed", async () => {
  const sessions = await AppRuntime.runPromise(Session.Service)
  const first = await AppRuntime.runPromise(sessions.scopeFence(SessionID.make("ses_scope_capacity_first")))
  for (let i = 0; i < 1024; i++)
    await AppRuntime.runPromise(sessions.scopeFence(SessionID.make("ses_scope_capacity_" + i)))
  expect(first()).toBe(false)
  expect((await AppRuntime.runPromise(sessions.scopeFence(SessionID.make("ses_scope_capacity_1023"))))()).toBe(true)
})

function request(socketPath: string, method: string, params: object) {
  return new Promise<{ result?: unknown; error?: { code: string } }>((resolve, reject) => {
    const socket = connect(socketPath)
    let input = ""
    socket.setTimeout(4000, () => socket.destroy(new Error("fixture timeout")))
    socket.on("error", reject)
    socket.on("connect", () => socket.write(JSON.stringify({ version: 1, id: randomUUID(), method, params }) + "\n"))
    socket.on("data", (chunk) => {
      input += chunk.toString("utf8")
    })
    socket.on("end", () => {
      try {
        resolve(JSON.parse(input))
      } catch (error) {
        reject(error)
      }
    })
  })
}

for (const operation of ["update", "delete"] as const) {
  test(`session B remote inspect/answer stays available while A ${operation} waits on an actual HTTP listener`, async () => {
    await using f = await fixture()
    const a = await f.run(f.sessions.create({}))
    const attention = (await f.native.decisions.attention(f.session.id))!
    const detail = (await f.native.decisions.detail(f.session.id, f.refs[0]))!
    const entered = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    const server = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      async fetch() {
        entered.resolve()
        await release.promise
        return new Response("synthetic listener only")
      },
    })
    const off = await f.run(
      EventV2Bridge.Service.use((events) =>
        events.listen((event) => {
          if (
            event.type !== (operation === "update" ? Session.Event.Updated.type : Session.Event.Deleted.type) ||
            (event.data as { sessionID: SessionID }).sessionID !== a.id
          )
            return Effect.void
          return Effect.promise(async () => {
            const response = await fetch(server.url, { method: "POST", signal: AbortSignal.timeout(5000) })
            await response.text()
          })
        }),
      ),
    )
    const ipc = await mkdtemp(path.join(process.env.DJINN_SHARING_TEST_ROOT!, "w"))
    const endpoint = await PromptControl.start({ ...f.native, sharing: f.sharing, socketPath: path.join(ipc, "s") })
    let finished = false
    const mutation = f
      .run(
        operation === "update"
          ? f.sessions.setTitle({ sessionID: a.id, title: "synthetic update" })
          : f.sessions.remove(a.id),
      )
      .then(() => {
        finished = true
      })
    try {
      await entered.promise
      expect(finished).toBe(false)
      expect(attention.current()).toBe(true)
      expect(detail.current()).toBe(true)
      const selectors = {
        expected_runtime_id: endpoint.runtimeID,
        session_id: f.session.id,
        grant_id: f.grant.grant_id,
      }
      expect((await request(path.join(ipc, "s"), "shared_attention", selectors)).error).toBeUndefined()
      const inspected = await request(path.join(ipc, "s"), "shared_decision", { ...selectors, ...f.refs[0] })
      expect(inspected.error).toBeUndefined()
      expect(inspected.result).toBeDefined()
      const answered = await request(path.join(ipc, "s"), "shared_decide", {
        ...selectors,
        ...f.refs[0],
        action: { type: "question_answer", answers: [["A"]] },
      })
      expect(answered.error).toBeUndefined()
      expect((answered.result as { type: string }).type).toBe("shared_decision_applied")
      expect(await f.run(f.questions.list())).toHaveLength(0)
      expect(await f.run(f.permissions.list())).toHaveLength(1)
      expect(finished).toBe(false)
    } finally {
      release.resolve()
      try {
        await mutation
      } finally {
        await f.run(off)
        await endpoint.close()
        await server.stop(true)
        await rm(ipc, { recursive: true, force: true })
        await f.run(f.sessions.remove(a.id).pipe(Effect.ignore))
      }
    }
  })
}

for (const method of ["shared_attention", "shared_decision"] as const) {
  test(`wire ${method} cannot serialize a sample held across actual workspace reassignment`, async () => {
    await using f = await fixture()
    const ipc = await mkdtemp(path.join(process.env.DJINN_SHARING_TEST_ROOT!, "w"))
    const entered = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    const endpoint = await PromptControl.start({
      ...f.native,
      sharing: f.sharing,
      socketPath: path.join(ipc, "s"),
      decisions: {
        ...f.native.decisions,
        async attention(sid) {
          const sample = await f.native.decisions.attention(sid)
          entered.resolve()
          await release.promise
          return sample
        },
        async detail(sid, ref) {
          const sample = await f.native.decisions.detail(sid, ref)
          entered.resolve()
          await release.promise
          return sample
        },
      },
    })
    const response = request(path.join(ipc, "s"), method, {
      expected_runtime_id: endpoint.runtimeID,
      session_id: f.session.id,
      grant_id: f.grant.grant_id,
      ...(method === "shared_decision" ? f.refs[0] : {}),
    })
    try {
      await entered.promise
      await f.run(f.sessions.setWorkspace({ sessionID: f.session.id, workspaceID }))
      expect(f.sharing.current(f.grant.grant_id, f.session.id, f.ctx)).toBe(true)
      release.resolve()
      const reply = await response
      expect(reply.result).toBeUndefined()
      expect(reply.error?.code).toBe("decision_stale")
      expect(await f.run(f.questions.list())).toHaveLength(1)
      expect(await f.run(f.permissions.list())).toHaveLength(1)
    } finally {
      release.resolve()
      try {
        await response
      } finally {
        try {
          await endpoint.close()
        } finally {
          await rm(ipc, { recursive: true, force: true })
        }
      }
    }
  })
}
