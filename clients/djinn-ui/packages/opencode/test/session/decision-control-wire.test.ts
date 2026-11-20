import { expect, test } from "bun:test"
import { randomUUID } from "node:crypto"
import { mkdir, mkdtemp, rename, rm } from "node:fs/promises"
import { connect } from "node:net"
import path from "node:path"
import { Effect, Fiber } from "effect"
import { AppRuntime } from "../../src/effect/app-runtime"
import { InstanceRef } from "../../src/effect/instance-ref"
import { InstanceStore } from "../../src/project/instance-store"
import { Question } from "../../src/question"
import { Permission } from "../../src/permission"
import { PermissionV1 } from "@opencode-ai/core/v1/permission"
import { Session } from "../../src/session/session"
import { PromptControlRuntime } from "../../src/session/prompt-control-runtime"
import { PromptControl } from "../../src/session/prompt-control"
import { DecisionControl } from "../../src/session/decision-control"
import { WebSharing } from "../../src/session/web-sharing"
import { tmpdir } from "../fixture/fixture"

function request(socketPath: string, method: string, params: object, id = randomUUID()) {
  return new Promise<{ result?: Record<string, unknown>; error?: { code: string; delivery: string } }>(
    (resolve, reject) => {
      const socket = connect(socketPath)
      let input = ""
      socket.setTimeout(4000, () => socket.destroy(new Error("fixture timeout")))
      socket.on("error", reject)
      socket.on("connect", () => socket.write(JSON.stringify({ version: 1, id, method, params }) + "\n"))
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
    },
  )
}

for (const { mode, enabled } of [
  { mode: "read", enabled: true },
  { mode: "read_write", enabled: true },
  { mode: "read_write", enabled: false },
] as const) {
  test(`actual automatic decision wire ${mode} opt-in=${enabled}: no model config, exact schemas and native settlement`, async () => {
    await using tmp = await tmpdir()
    const ipc = await mkdtemp(path.join(process.env.DJINN_SHARING_TEST_ROOT!, "w"))
    const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
    const run = <A, E>(effect: Effect.Effect<A, E, Question.Service | Permission.Service | Session.Service>) =>
      AppRuntime.runPromise(effect.pipe(Effect.provideService(InstanceRef, ctx)))
    const session = await run(Session.Service.use((service) => service.create({})))
    const native = PromptControlRuntime.automaticCallbacks(tmp.path, enabled)
    const sharing = WebSharing.create({ directory: tmp.path, mode, qualify: native.qualify })
    let lostAck = false
    let invalidateDetail = false
    const endpoint = await PromptControl.start({
      socketPath: path.join(ipc, "s"),
      sharing,
      ...native,
      decisions: {
        ...native.decisions,
        async detail(sid, ref) {
          const sample = await native.decisions.detail(sid, ref)
          if (invalidateDetail)
            await run(Question.Service.use((service) => service.reject(ref.request_id as Question.Request["id"])))
          return sample
        },
        async decide(input) {
          const result = await native.decisions.decide(input)
          if (lostAck && result === "applied") throw new Error("synthetic lost receipt")
          return result
        },
      },
    })
    const fibers: Fiber.Fiber<unknown, unknown>[] = []
    const ask = async () => {
      fibers.push(
        AppRuntime.runFork(
          Question.Service.use((service) =>
            service.ask({
              sessionID: session.id,
              questions: [
                {
                  question: "Fixture?",
                  header: "Choice",
                  custom: false,
                  options: [{ label: "A", description: "Reviewed" }],
                },
              ],
            }),
          ).pipe(Effect.provideService(InstanceRef, ctx)),
        ),
      )
      return run(
        Effect.gen(function* () {
          const service = yield* Question.Service
          for (;;) {
            const meta = (yield* service.attention(session.id)).value.requests[0]
            if (meta) return meta as DecisionControl.Reference
            yield* Effect.yieldNow
          }
        }).pipe(Effect.timeout("2 seconds")),
      )
    }
    try {
      const cap = await request(path.join(ipc, "s"), "capabilities", {})
      expect(cap.result?.decision_control).toEqual(DecisionControl.CAPABILITY)
      await sharing.select({
        frontend_id: randomUUID(),
        revision: 1,
        selection_generation: 1,
        selection: { sessionID: session.id, directory: tmp.path },
      })
      const grant = (await sharing.sample())!
      expect(grant.can_submit).toBe(false)
      const selectors = { expected_runtime_id: endpoint.runtimeID, session_id: session.id, grant_id: grant.grant_id }
      const meta = await ask()
      const target = {
        ...selectors,
        kind: meta.kind,
        request_id: meta.request_id,
        occurrence_id: meta.occurrence_id,
        details_sha256: meta.details_sha256,
      }
      const attention = await request(path.join(ipc, "s"), "shared_attention", selectors)
      expect(attention.result?.can_respond).toBe(mode === "read_write" && enabled)
      expect(attention.result?.requests).toEqual([meta])
      const detail = await request(path.join(ipc, "s"), "shared_decision", target)
      expect((detail.result?.decision as DecisionControl.Detail).details_sha256).toBe(meta.details_sha256)
      const attempt = randomUUID()
      const action = { type: "question_answer", answers: [["A"]] }
      if (mode === "read" || !enabled) {
        expect((await request(path.join(ipc, "s"), "shared_decide", { ...target, action }, attempt)).error?.code).toBe(
          mode === "read" ? "sharing_read_only" : "decision_unavailable",
        )
        expect((await run(Question.Service.use((service) => service.list()))).length).toBe(1)
      } else {
        for (const action of [
          { type: "permission_always" },
          { type: "question_answer", answers: [["bad"]] },
          { type: "question_reject", extra: true },
        ]) {
          expect((await request(path.join(ipc, "s"), "shared_decide", { ...target, action })).error?.code).toBe(
            "invalid_decision",
          )
        }
        expect((await request(path.join(ipc, "s"), "shared_decide", { ...target, action }, attempt)).result?.type).toBe(
          "shared_decision_applied",
        )
        expect(await AppRuntime.runPromise(Fiber.join(fibers[0]))).toEqual([["A"]])
        expect(
          (await request(path.join(ipc, "s"), "shared_submit", { ...selectors, text: "never admitted" }, attempt)).error
            ?.code,
        ).toBe("request_already_seen")
        const fresh = await ask()
        lostAck = true
        const uncertain = await request(path.join(ipc, "s"), "shared_decide", {
          ...selectors,
          ...fresh,
          details_available: undefined,
          action,
        })
        expect(uncertain.error).toEqual({ code: "decision_uncertain", delivery: "uncertain" })
        expect(await AppRuntime.runPromise(Fiber.join(fibers[1]))).toEqual([["A"]])
        lostAck = false
        const cancelled = await ask()
        invalidateDetail = true
        expect(
          (
            await request(path.join(ipc, "s"), "shared_decision", {
              ...selectors,
              ...cancelled,
              details_available: undefined,
            })
          ).error?.code,
        ).toBe("decision_stale")
        invalidateDetail = false
      }
      await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.dispose(ctx)))
      expect((await request(path.join(ipc, "s"), "shared_attention", selectors)).error?.code).toBe("grant_unavailable")
    } finally {
      sharing.close()
      await endpoint.close()
      for (const fiber of fibers) await AppRuntime.runPromise(Fiber.interrupt(fiber))
      await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.disposeDirectory(tmp.path)))
      await run(Session.Service.use((service) => service.remove(session.id)))
      await rm(ipc, { recursive: true, force: true })
    }
  })
}

test("standalone actual producer fixture: counts, conditional waits, lost acknowledgement and readmode", async () => {
  const child = Bun.spawn(
    [process.execPath, path.join(import.meta.dir, "decision-control.fixture.ts"), process.env.DJINN_SHARING_TEST_ROOT!],
    {
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
      timeout: 20000,
    },
  )
  const reader = child.stdout.getReader()
  let buffer = ""
  const stderr = new Response(child.stderr).text()
  async function line(type: string): Promise<Record<string, unknown>> {
    for (;;) {
      while (!buffer.includes("\n")) {
        const next = await reader.read()
        if (next.done) throw new Error("fixture ended before " + type + ": " + (await stderr))
        buffer += new TextDecoder().decode(next.value)
      }
      const index = buffer.indexOf("\n")
      const text = buffer.slice(0, index)
      buffer = buffer.slice(index + 1)
      const value = JSON.parse(text) as Record<string, unknown>
      if (value.type === "control_error") throw new Error("fixture control failed")
      if (value.type === type) return value
    }
  }
  const control = async (command: string, value?: boolean) => {
    child.stdin.write(JSON.stringify({ command, value }) + "\n")
    return line("control")
  }
  try {
    const ready = await line("ready")
    expect(ready.synthetic).toBe(true)
    expect(ready.pending).toBe(2)
    expect(ready.settled).toBe(0)
    const socket = ready.socket as string
    const pin = { expected_runtime_id: ready.runtime_id }
    await control("select")
    const status = await request(socket, "share_status", pin)
    const grant = status.result?.grant as { grant_id: string; session_id: string; can_submit: boolean }
    expect(grant.can_submit).toBe(false)
    const selectors = { ...pin, session_id: grant.session_id, grant_id: grant.grant_id }
    const attention = await request(socket, "shared_attention", selectors)
    const requests = attention.result?.requests as DecisionControl.Meta[]
    const q = requests.find((r) => r.kind === "question")!
    const p = requests.find((r) => r.kind === "permission")!
    const target = (meta: DecisionControl.Meta) => ({
      ...selectors,
      kind: meta.kind,
      request_id: meta.request_id,
      occurrence_id: meta.occurrence_id,
      details_sha256: meta.details_sha256,
    })
    expect(
      (await request(socket, "shared_decide", { ...target(q), action: { type: "question_answer", answers: [["A"]] } }))
        .result?.type,
    ).toBe("shared_decision_applied")
    expect((await control("counts")).settled).toBe(1)
    await control("readmode", true)
    expect(
      (await request(socket, "shared_decide", { ...target(p), action: { type: "permission_once" } })).error?.code,
    ).toBe("sharing_read_only")
    await control("readmode", false)
    await control("lost_ack", true)
    const attempt = randomUUID()
    expect(
      (await request(socket, "shared_decide", { ...target(p), action: { type: "permission_once" } }, attempt)).error
        ?.code,
    ).toBe("decision_uncertain")
    expect((await control("counts")).settled).toBe(2)
    expect((await control("permission")).pending).toBe(1)
    const old = ((await request(socket, "shared_attention", selectors)).result?.requests as DecisionControl.Meta[])[0]
    const replacement = await control("replace")
    expect(replacement.pending).toBe(1)
    expect(replacement.rejected).toBe(1)
    expect((await request(socket, "shared_decision", target(old))).error?.code).toBe("decision_stale")
    expect((await control("cancel")).pending).toBe(0)
    child.stdin.end()
    expect(await child.exited).toBe(0)
  } finally {
    reader.releaseLock()
    child.kill()
    await child.exited
    await stderr
  }
})

for (const change of ["selection", "reload", "root"] as const) {
  test(`actual automatic claim refuses ${change} changes without loading a second instance`, async () => {
    await using tmp = await tmpdir()
    const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
    const session = await AppRuntime.runPromise(
      Session.Service.use((service) => service.create({})).pipe(Effect.provideService(InstanceRef, ctx)),
    )
    const native = PromptControlRuntime.automaticCallbacks(tmp.path, true)
    const sharing = WebSharing.create({ directory: tmp.path, mode: "read_write", qualify: native.qualify })
    const frontend_id = randomUUID()
    await sharing.select({
      frontend_id,
      revision: 1,
      selection_generation: 1,
      selection: { sessionID: session.id, directory: tmp.path },
    })
    const grant = (await sharing.sample())!
    const fiber = AppRuntime.runFork(
      Permission.Service.use((service) =>
        service.ask({
          id: PermissionV1.ID.make("per_fence"),
          sessionID: session.id,
          permission: "fixture",
          patterns: ["synthetic"],
          always: [],
          metadata: {},
          ruleset: [],
        }),
      ).pipe(Effect.provideService(InstanceRef, ctx)),
    )
    let moved = false
    let claims = 0
    try {
      const meta = await AppRuntime.runPromise(
        Effect.gen(function* () {
          const service = yield* Permission.Service
          for (;;) {
            const meta = (yield* service.attention(session.id)).value.requests[0]
            if (meta) return meta as DecisionControl.Reference
            yield* Effect.yieldNow
          }
        }).pipe(Effect.timeout("2 seconds"), Effect.provideService(InstanceRef, ctx)),
      )
      const sample = await native.decisions.detail(session.id, meta)
      if (change === "selection")
        await sharing.select({ frontend_id, revision: 2, selection_generation: 2, selection: null })
      if (change === "reload")
        await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.reload({ directory: tmp.path })))
      if (change === "root") {
        await rename(tmp.path, tmp.path + ".saved")
        moved = true
        await mkdir(tmp.path)
      }
      expect(
        await native.decisions.decide({
          ...meta,
          session_id: session.id,
          action: { type: "permission_once" },
          current: (context) => sharing.current(grant.grant_id, session.id, context),
          onClaim: () => {
            claims++
          },
        }),
      ).toBe("decision_stale")
      expect(claims).toBe(0)
      if (change !== "selection") expect(sample?.current()).toBe(false)
      const active = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.peek(tmp.path)))
      if (change !== "reload") expect(active).toBe(ctx)
      if (change === "selection") {
        expect(
          (
            await AppRuntime.runPromise(
              Permission.Service.use((service) => service.list()).pipe(Effect.provideService(InstanceRef, ctx)),
            )
          ).length,
        ).toBe(1)
      }
    } finally {
      sharing.close()
      if (moved) {
        await rm(tmp.path, { recursive: true, force: true })
        await rename(tmp.path + ".saved", tmp.path)
      }
      await AppRuntime.runPromise(Fiber.interrupt(fiber))
      await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.disposeDirectory(tmp.path)))
      await AppRuntime.runPromise(
        Session.Service.use((service) => service.remove(session.id)).pipe(Effect.provideService(InstanceRef, ctx)),
      )
    }
  })
}
