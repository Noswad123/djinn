import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { randomUUID } from "node:crypto"
import { FSUtil } from "@opencode-ai/core/fs-util"
import { WorkspaceV2 } from "@opencode-ai/core/workspace"
import { Clock, Context, Deferred, Effect, Exit, Layer, Scope, ScopedCache } from "effect"
import { GlobalBus, type GlobalEvent } from "@/bus/global"
import { InstanceRef, WorkspaceRef } from "@/effect/instance-ref"
import { InstanceState } from "@/effect/instance-state"
import { registerDisposer } from "@/effect/instance-registry"
import { RuntimeFlags } from "@/effect/runtime-flags"
import { EffectBridge } from "@/effect/bridge"
import { InstanceStore } from "@/project/instance-store"
import { Permission } from "@/permission"
import { Question } from "@/question"
import { Session } from "@/session/session"
import { SessionID } from "@/session/schema"
import { SessionStatus } from "@/session/status"
import { RunLifecycle } from "@/session/run-lifecycle"
import { ServerAuth } from "@/server/auth"
import { NumenTransport, type Activity } from "./transport"
import { LiveAttach } from "./live-attach"

export interface Input {
  viewer: string
  revision: number
  selection?: { sessionID: string; directory: string; workspaceID?: string }
}

export interface Interface {
  readonly select: (input: Input) => Effect.Effect<void>
  readonly resolve: (input: LiveAttach.Query) => LiveAttach.Target | undefined
  readonly reset: () => Effect.Effect<void>
}

export class Service extends Context.Service<Service, Interface>()("@opencode/Numen") {}

// A separate boundary keeps tests off the socket without mocking session services.
export class Transport extends Context.Service<Transport, { readonly create: typeof NumenTransport.create }>()(
  "@opencode/NumenTransport",
) {}

const transportLayer = Layer.succeed(Transport, { create: NumenTransport.create })
const transportNode = LayerNode.make({ service: Transport, layer: transportLayer, deps: [] })

type Viewer = { revision: number; expires: number; token: object; directory?: string; record?: RecordState }
type RecordState = {
  id: SessionID
  incarnation: string
  workspace?: string
  directory: string
  ready: boolean
  initialized: Deferred.Deferred<void>
  summary: string
  status: SessionStatus.Info
  expectedStatus?: SessionStatus.Info["type"]
  permissions: Set<string>
  questions: Set<string>
  generation: number
  dirty: boolean
  alive: boolean
  observed: boolean
  run?: RunLifecycle.Event
  terminal?: { state: "succeeded" | "failed" | "cancelled"; reason: string; expires: number }
  owner: Set<RecordState>
}

export const layer = Layer.effect(
  Service,
  Effect.gen(function* () {
    const flags = yield* RuntimeFlags.Service
    const factory = yield* Transport
    const store = yield* InstanceStore.Service
    const sessions = yield* Session.Service
    const statuses = yield* SessionStatus.Service
    const permissions = yield* Permission.Service
    const questions = yield* Question.Service
    const lifecycle = yield* RunLifecycle.Service
    const bridge = yield* EffectBridge.make()
    const viewers = new Map<string, Viewer>()
    // Released/expired viewers do not consume active capacity. Retain a bounded
    // one-minute replay fence; pending acquisitions also require object identity.
    const tombstones = new Map<string, { revision: number; expires: number }>()
    const retire = (id: string, revision: number, now: number) => {
      const viewer = viewers.get(id)
      if (viewer) viewer.token = {}
      viewers.delete(id)
      tombstones.delete(id)
      tombstones.set(id, { revision, expires: now + 60_000 })
      if (tombstones.size > 256) tombstones.delete(tombstones.keys().next().value!)
    }
    const expire = (now: number) => {
      for (const [id, item] of tombstones) if (item.expires <= now) tombstones.delete(id)
      for (const [id, viewer] of viewers) if (viewer.expires <= now) retire(id, viewer.revision, now)
    }
    const records = new Set<RecordState>()
    // Exact session pins require stable IDs: fail closed after 32 per transport lifetime.
    const admitted = new Set<SessionID>()
    const transitions: { record: RecordState; activity: Activity; expires: number; connection: number | null }[] = []
    let suppressTransitions = false
    let epoch = 0
    let resetting = false
    let offDispose: (() => void) | undefined
    let publisher:
      | {
          instance: string
          scope: Scope.Closeable
          transport: ReturnType<typeof NumenTransport.create>
          gateway?: ReturnType<typeof LiveAttach.start>
          snapshot?: string
        }
      | undefined

    const viewed = (record: RecordState) => [...viewers.values()].some((viewer) => viewer.record === record)
    const busy = (record: RecordState) =>
      record.run?.phase === "owned" ||
      record.status.type !== "idle" ||
      record.permissions.size > 0 ||
      record.questions.size > 0
    const remove = (record: RecordState) => {
      record.alive = false
      records.delete(record)
      record.owner.delete(record)
      for (const viewer of viewers.values()) if (viewer.record === record) viewer.record = undefined
    }
    const activity = (record: RecordState): Activity => {
      if (record.terminal && !busy(record))
        return {
          activity: record.id,
          summary: record.summary,
          state: record.terminal.state,
          reason: record.terminal.reason,
        }
      return {
        activity: record.id,
        summary: record.summary,
        state:
          record.permissions.size || record.questions.size || record.status.type === "retry"
            ? "waiting"
            : busy(record)
              ? "working"
              : "idle",
        ...(record.permissions.size || record.questions.size ? { attention: true, reason: "Input required" } : {}),
        ...(record.status.type === "retry" && !record.permissions.size && !record.questions.size
          ? { attention: false, reason: "retry" }
          : {}),
      }
    }
    const publish = () => {
      drain()
      for (const record of records)
        if (
          !viewed(record) &&
          !(record.observed && busy(record)) &&
          !(record.terminal && record.terminal.expires > performance.now())
        )
          remove(record)
      const activities = [...records].filter((record) => record.ready).map(activity)
      const snapshot = JSON.stringify(activities)
      if (!publisher || publisher.snapshot === snapshot) return
      publisher.snapshot = snapshot
      publisher.transport.replace(activities)
    }
    const drain = () => {
      // A lost prefix must not leave a later terminal transition behind it.
      if (transitions.some((entry) => entry.expires <= performance.now())) transitions.length = 0
      for (const entry of transitions.splice(0)) {
        if (!entry.record.alive || !entry.record.ready || !publisher) continue
        publisher.transport.transition(entry.activity, entry.connection)
        // A transition mutates transport state independently of the snapshot cache.
        publisher.snapshot = undefined
      }
      suppressTransitions = false
    }

    const state = yield* InstanceState.make((ctx) =>
      Effect.gen(function* () {
        const owned = new Set<RecordState>()
        const local = { alive: true, owned, pending: 0 }
        const offLifecycle = lifecycle.subscribe((event) => {
          if (
            !local.alive ||
            event.kind !== "response" ||
            event.directory !== ctx.directory ||
            event.projectID !== ctx.project.id
          )
            return
          for (const record of owned) {
            if (!record.alive || record.id !== event.sessionID || record.workspace !== event.workspaceID) continue
            if (event.phase === "owned") {
              // Ownership order, not settlement arrival order, decides which run is current.
              const current = lifecycle.current(event)
              if (current?.runID !== event.runID || current.instanceID !== event.instanceID) continue
              if (
                record.run?.instanceID === event.instanceID &&
                (record.run.order > event.order || record.run.runID === event.runID)
              )
                continue
              record.run = event
              record.terminal = undefined
              record.observed = true
            } else {
              if (
                record.run?.phase !== "owned" ||
                record.run.instanceID !== event.instanceID ||
                record.run.runID !== event.runID ||
                record.run.order !== event.order
              )
                continue
              record.run = event
              record.status = { type: "idle" }
              record.expectedStatus = "idle"
              record.permissions.clear()
              record.questions.clear()
              record.terminal =
                event.outcome === "completed" || event.outcome === "failed" || event.outcome === "cancelled"
                  ? {
                      state: event.outcome === "completed" ? "succeeded" : event.outcome,
                      reason: event.outcome === "completed" ? "response_completed" : `run_${event.outcome}`,
                      // Allow the bounded live queue and 1.5-second cue to finish before retiring an unviewed target.
                      expires: performance.now() + 5000,
                    }
                  : undefined
            }
            record.generation++
            record.dirty = true
            if (!record.ready || !publisher || suppressTransitions) continue
            if (transitions.length === 32) {
              transitions.length = 0
              suppressTransitions = true
              continue
            }
            transitions.push({
              record,
              activity: activity(record),
              expires: performance.now() + 2000,
              connection: publisher.transport.connection() ?? null,
            })
          }
        })
        const onEvent = (event: GlobalEvent) => {
          if (!local.alive || event.directory !== ctx.directory || (event.project && event.project !== ctx.project.id))
            return
          const payload: {
            type?: string
            properties?: {
              sessionID?: string
              status?: SessionStatus.Info
              id?: string
              requestID?: string
              info?: { id: string }
            }
          } = event.payload
          const data = payload.properties
          const sessionID = payload.type === "session.deleted" ? data?.info?.id : data?.sessionID
          if (!data || !sessionID) return
          for (const record of owned) {
            if (!record.alive || record.id !== sessionID || record.workspace !== event.workspace) continue
            if (payload.type === "session.status" && data.status) {
              record.status = data.status
              record.expectedStatus = data.status.type
              if (data.status.type !== "idle") record.terminal = undefined
            } else if (payload.type === "permission.asked" && data.id) record.permissions.add(data.id)
            else if (payload.type === "permission.replied" && data.requestID) record.permissions.delete(data.requestID)
            else if (payload.type === "question.asked" && data.id) record.questions.add(data.id)
            else if ((payload.type === "question.replied" || payload.type === "question.rejected") && data.requestID)
              record.questions.delete(data.requestID)
            else if (payload.type === "session.deleted") remove(record)
            else continue
            if (viewed(record) && busy(record)) record.observed = true
            record.generation++
            record.dirty = true
          }
        }
        GlobalBus.on("event", onEvent)
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            local.alive = false
            offLifecycle()
            GlobalBus.off("event", onEvent)
            for (const record of owned) remove(record)
            owned.clear()
            publish()
          }),
        )

        const reconcile = Effect.gen(function* () {
          for (const record of owned) {
            if (!record.ready) continue
            if (!record.alive) {
              owned.delete(record)
              continue
            }
            // Let the publisher finish its map mutation before authoritative reads.
            if (record.dirty) {
              record.dirty = false
              continue
            }
            const generation = record.generation
            yield* Effect.gen(function* () {
              const info = yield* sessions.get(record.id).pipe(Effect.option)
              const status = yield* statuses.get(record.id)
              const pendingPermissions = yield* permissions.list()
              const pendingQuestions = yield* questions.list()
              if (!local.alive || !record.alive || record.generation !== generation) return
              if (
                info._tag === "None" ||
                info.value.directory !== ctx.directory ||
                info.value.projectID !== ctx.project.id ||
                info.value.workspaceID !== record.workspace
              ) {
                remove(record)
                return
              }
              record.summary = info.value.title
              // publish() may still be suspended in another event listener.
              if (!record.expectedStatus || record.expectedStatus === status.type) {
                record.status = status
                record.expectedStatus = undefined
              }
              if (record.status.type !== "idle") record.terminal = undefined
              record.permissions = new Set(
                pendingPermissions.filter((item) => item.sessionID === record.id).map((item) => item.id),
              )
              record.questions = new Set(
                pendingQuestions.filter((item) => item.sessionID === record.id).map((item) => item.id),
              )
              if (viewed(record) && busy(record)) record.observed = true
            }).pipe(
              Effect.provideService(WorkspaceRef, record.workspace ? WorkspaceV2.ID.make(record.workspace) : undefined),
            )
          }
          publish()
        }).pipe(Effect.provideService(InstanceRef, ctx))
        yield* Effect.gen(function* () {
          while (true) {
            yield* Effect.sleep("250 millis")
            yield* reconcile
          }
        }).pipe(Effect.forkScoped)
        return local
      }),
    )

    const prune = Effect.gen(function* () {
      for (const [directory] of yield* ScopedCache.entries(state.cache)) {
        yield* ScopedCache.invalidateWhen(state.cache, directory, (local) => {
          if (local.owned.size || local.pending) return false
          local.alive = false
          return true
        })
      }
    })

    const resetOnce = Effect.fn("Numen.reset")(function* () {
      epoch++
      resetting = true
      offDispose?.()
      offDispose = undefined
      viewers.clear()
      tombstones.clear()
      transitions.length = 0
      suppressTransitions = false
      for (const record of records) record.alive = false
      records.clear()
      const previous = publisher
      publisher = undefined
      if (previous?.gateway) yield* Effect.promise(() => previous.gateway!.close().catch(() => {}))
      previous?.transport.close()
      admitted.clear()
      if (previous) yield* Scope.close(previous.scope, Exit.void)
      yield* ScopedCache.invalidateAll(state.cache)
      resetting = false
    }, Effect.uninterruptible)
    const resettingEffect = yield* Effect.cachedWithTTL(resetOnce(), 0)
    const reset = () => resettingEffect
    yield* Effect.addFinalizer(() => reset())

    const resolve = (input: LiveAttach.Query): LiveAttach.Target | undefined => {
      if (
        resetting ||
        !publisher ||
        input.instance !== publisher.instance ||
        (input.generation !== undefined && input.generation !== publisher.gateway?.generation)
      )
        return
      const matches = [...records].filter(
        (record) =>
          record.alive &&
          record.ready &&
          record.id === input.session &&
          (input.incarnation === undefined || input.incarnation === record.incarnation),
      )
      // An ambiguous session placement cannot be attached without an incarnation pin.
      if (matches.length !== 1) return
      const record = matches[0]!
      return {
        instance: publisher.instance,
        session: record.id,
        incarnation: record.incarnation,
        directory: record.directory,
        ...(record.workspace ? { workspaceID: record.workspace } : {}),
      }
    }

    const select = Effect.fn("Numen.select")(function* (input: Input, expected?: LiveAttach.Target) {
      if (
        resetting ||
        !flags.numen ||
        flags.numenDisabled ||
        !input.viewer ||
        input.viewer.length > 128 ||
        !Number.isSafeInteger(input.revision) ||
        input.revision < 0
      )
        return
      const stamp = epoch
      const now = yield* Clock.currentTimeMillis
      if (resetting || epoch !== stamp) return
      expire(now)
      if (expected) {
        const found = resolve(expected)
        if (
          !found ||
          found.directory !== expected.directory ||
          found.workspaceID !== expected.workspaceID ||
          (input.selection &&
            (input.selection.sessionID !== found.session ||
              input.selection.directory !== found.directory ||
              input.selection.workspaceID !== found.workspaceID))
        )
          return
      }
      const previous = viewers.get(input.viewer)
      if (input.revision <= (previous?.revision ?? tombstones.get(input.viewer)?.revision ?? -1)) return
      if (!input.selection) {
        retire(input.viewer, input.revision, now)
        publish()
        yield* prune
        return
      }
      if (!previous && viewers.size >= 32) return
      tombstones.delete(input.viewer)
      const token = {}
      const selection = input.selection
      const directory = selection ? FSUtil.resolve(selection.directory) : undefined
      const viewer: Viewer = {
        revision: input.revision,
        expires: now + 20_000,
        token,
        directory,
        record: selection
          ? [...records].find(
              (record) =>
                record.alive &&
                record.id === selection.sessionID &&
                record.directory === directory &&
                record.workspace === selection.workspaceID,
            )
          : undefined,
      }
      viewers.set(input.viewer, viewer)
      const current = () => epoch === stamp && viewers.get(input.viewer) === viewer && viewer.token === token
      // Authenticated live viewers may only lease an existing ready record. Never load or recreate it.
      if (expected) {
        publish()
        return
      }
      // Invalidate acquisitions even if disposal wins before their InstanceState lookup.
      offDispose ??= registerDisposer((directory) => {
        for (const viewer of viewers.values()) if (viewer.directory === directory) viewer.token = {}
        return Promise.resolve()
      })
      // All stamps are installed before loading an instance, which can suspend.
      yield* store
        .provide(
          { directory: selection.directory },
          Effect.gen(function* () {
            if (!current()) return
            const ctx = yield* InstanceState.context
            const local = yield* InstanceState.get(state)
            if (!current() || !local.alive) return
            local.pending++
            yield* Effect.gen(function* () {
              const info = yield* sessions.get(SessionID.make(selection.sessionID)).pipe(Effect.option)
              const completed = yield* Clock.currentTimeMillis
              if (
                !current() ||
                !local.alive ||
                completed >= viewer.expires ||
                info._tag === "None" ||
                info.value.directory !== ctx.directory ||
                info.value.projectID !== ctx.project.id ||
                info.value.workspaceID !== selection.workspaceID
              )
                return
              const existing = [...local.owned].find(
                (record) => record.alive && record.id === info.value.id && record.workspace === selection.workspaceID,
              )
              if (existing) {
                viewer.record = existing
                yield* Deferred.await(existing.initialized)
                if (!current() || !local.alive || !existing.alive || !existing.ready) return
                viewer.record = existing
                publish()
                return
              }
              publish()
              if (records.size >= 16 || (!admitted.has(info.value.id) && admitted.size >= 32)) return
              const record: RecordState = {
                id: info.value.id,
                incarnation: randomUUID(),
                directory: ctx.directory,
                workspace: selection.workspaceID,
                ready: false,
                initialized: Deferred.makeUnsafe<void>(),
                summary: info.value.title,
                status: { type: "idle" },
                permissions: new Set(),
                questions: new Set(),
                generation: 0,
                dirty: false,
                alive: true,
                observed: false,
                run: lifecycle.current({
                  directory: ctx.directory,
                  sessionID: info.value.id,
                  workspaceID: selection.workspaceID,
                }),
                owner: local.owned,
              }
              local.owned.add(record)
              records.add(record)
              viewer.record = record
              yield* Effect.gen(function* () {
                while (true) {
                  const generation = record.generation
                  // The first lookup can have captured a session before deletion,
                  // before this record was visible to the deletion subscription.
                  const currentInfo = yield* sessions.get(record.id).pipe(Effect.option)
                  const status = yield* statuses.get(record.id)
                  const pendingPermissions = yield* permissions.list()
                  const pendingQuestions = yield* questions.list()
                  const now = yield* Clock.currentTimeMillis
                  // Initialization belongs to the record's current leases, not its first caller.
                  if (
                    epoch !== stamp ||
                    !local.alive ||
                    !record.alive ||
                    currentInfo._tag === "None" ||
                    currentInfo.value.directory !== ctx.directory ||
                    currentInfo.value.projectID !== ctx.project.id ||
                    currentInfo.value.workspaceID !== record.workspace ||
                    ![...viewers.values()].some((viewer) => viewer.record === record && viewer.expires > now)
                  )
                    return
                  if (record.generation !== generation) continue
                  if (!record.expectedStatus || record.expectedStatus === status.type) {
                    record.status = status
                    record.expectedStatus = undefined
                  }
                  record.permissions = new Set(
                    pendingPermissions.filter((item) => item.sessionID === record.id).map((item) => item.id),
                  )
                  record.questions = new Set(
                    pendingQuestions.filter((item) => item.sessionID === record.id).map((item) => item.id),
                  )
                  if (!admitted.has(record.id) && admitted.size >= 32) return
                  admitted.add(record.id)
                  record.observed = busy(record)
                  record.ready = true
                  return
                }
              }).pipe(
                Effect.ensuring(
                  Effect.sync(() => {
                    if (!record.ready) remove(record)
                    Deferred.doneUnsafe(record.initialized, Effect.void)
                  }),
                ),
              )
              if (!record.ready || !record.alive || !local.alive || epoch !== stamp || !viewed(record)) return
              if (!publisher) {
                const scope = Scope.makeUnsafe()
                const instance = randomUUID()
                publisher = {
                  instance,
                  scope,
                  transport: factory.create({
                    instance,
                    socket: flags.numenSocket || NumenTransport.defaultSocket(flags.numenRuntimeDir),
                  }),
                }
                const active = publisher
                try {
                  // Default-enabled ordinary chat selection creates this private
                  // backend-wide transport and auth record, not a session sandbox.
                  active.gateway = LiveAttach.start({
                    instance,
                    runtimeDir: flags.numenRuntimeDir || undefined,
                    resolve,
                    viewer: (target, input) =>
                      bridge.promise(
                        Effect.gen(function* () {
                          if (publisher !== active || !resolve(target)) return false
                          yield* select(input, target)
                          const lease = viewers.get(input.viewer)
                          return (
                            publisher === active &&
                            (input.selection
                              ? lease?.revision === input.revision && lease.record?.incarnation === target.incarnation
                              : tombstones.get(input.viewer)?.revision === input.revision)
                          )
                        }),
                      ),
                    fetch: async (request) => {
                      const { Server } = await import("@/server/server")
                      if (publisher !== active || request.signal.aborted) return new Response(null, { status: 503 })
                      return Server.Default().app.fetch(request)
                    },
                    authorization: () => ServerAuth.header(),
                  })
                } catch {
                  /* Optional attach failures must not affect the publisher or ordinary UI. */
                }
                yield* Scope.addFinalizer(
                  scope,
                  Effect.promise(async () => {
                    await active.gateway?.close().catch(() => {})
                  }),
                )
                yield* Effect.gen(function* () {
                  while (true) {
                    yield* Effect.sleep("250 millis")
                    const now = yield* Clock.currentTimeMillis
                    expire(now)
                    publish()
                    yield* prune
                  }
                }).pipe(Effect.forkIn(scope))
              }
              publish()
            }).pipe(
              Effect.ensuring(
                Effect.sync(() => {
                  local.pending--
                }),
              ),
            )
          }),
        )
        .pipe(
          Effect.provideService(
            WorkspaceRef,
            selection.workspaceID ? WorkspaceV2.ID.make(selection.workspaceID) : undefined,
          ),
          Effect.ensuring(prune),
        )
    })

    return Service.of({ select, resolve, reset })
  }),
)

export const node = LayerNode.make({
  service: Service,
  layer,
  deps: [
    RuntimeFlags.node,
    transportNode,
    InstanceStore.node,
    Session.node,
    SessionStatus.node,
    Permission.node,
    Question.node,
    RunLifecycle.node,
  ],
})

export * as Numen from "./service"
