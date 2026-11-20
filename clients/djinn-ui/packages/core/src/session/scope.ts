import { Context, Effect } from "effect"

type Entry = { token: object; active: number }
type Domain = { entries: Map<string, Entry>; overflow: number; closed: boolean }
type Transaction = {
  domain: Domain
  parent?: Transaction
  held: Map<string, () => void>
  overflow?: () => void
  active: boolean
}
const transactions = Context.Reference<Transaction | undefined>("@opencode/SessionScope/Transaction", {
  defaultValue: () => undefined,
})
const domains = new WeakMap<object, Domain>()
const CAPACITY = 1024

function domain(database: object) {
  const existing = domains.get(database)
  if (existing) return existing
  const created: Domain = { entries: new Map(), overflow: 0, closed: false }
  domains.set(database, created)
  return created
}

function entry(state: Domain, sessionID: string) {
  const existing = state.entries.get(sessionID)
  if (existing) return existing
  if (state.entries.size >= CAPACITY)
    for (const [id, value] of state.entries) {
      if (value.active) continue
      state.entries.delete(id)
      break
    }
  if (state.entries.size >= CAPACITY) return undefined
  const created = { token: {}, active: 0 }
  state.entries.set(sessionID, created)
  return created
}

function overflow(state: Domain) {
  state.overflow++
  return () => {
    state.overflow--
  }
}

function begin(state: Domain, sessionID: string) {
  const current = entry(state, sessionID)
  // Never evict active markers or prevent a legacy mutation at capacity.
  if (!current) return overflow(state)
  current.active++
  current.token = {}
  return () => {
    current.token = {}
    if (--current.active === 0) state.entries.delete(sessionID)
  }
}

/** Capture before any asynchronous eligibility read; the returned guard never yields. */
export function capture(database: object, sessionID: string): () => boolean {
  const state = domain(database)
  if (state.closed || state.overflow) return () => false
  const current = entry(state, sessionID)
  if (!current || current.active) return () => false
  const token = current.token
  return () =>
    !state.closed &&
    !state.overflow &&
    !current.active &&
    state.entries.get(sessionID) === current &&
    current.token === token
}

export function mutate<A, E, R>(database: object, sessionID: string, effect: Effect.Effect<A, E, R>) {
  return Effect.gen(function* () {
    const state = domain(database)
    let transaction = yield* transactions
    while (transaction && (!transaction.active || transaction.domain !== state)) transaction = transaction.parent
    const owner = transaction
    return yield* Effect.acquireUseRelease(
      Effect.sync(() => {
        const release = begin(state, sessionID)
        if (owner?.active && !owner.held.has(sessionID)) {
          if (owner.held.size < CAPACITY) owner.held.set(sessionID, begin(state, sessionID))
          else owner.overflow ??= overflow(state)
        }
        return release
      }),
      () => effect,
      (release) => Effect.sync(release),
    )
  })
}

/** Retain projection markers until the outermost transaction has committed or rolled back. */
export function transaction<A, E, R>(database: object, effect: Effect.Effect<A, E, R>) {
  return Effect.gen(function* () {
    const state = domain(database)
    const parent = yield* transactions
    let existing = parent
    while (existing && (!existing.active || existing.domain !== state)) existing = existing.parent
    if (existing) return yield* effect
    const owner: Transaction = { domain: state, parent, held: new Map(), active: true }
    return yield* effect.pipe(
      Effect.provideService(transactions, owner),
      Effect.ensuring(
        Effect.sync(() => {
          owner.active = false
          for (const release of owner.held.values()) release()
          owner.overflow?.()
          owner.held.clear()
        }),
      ),
    )
  })
}

export function close(database: object) {
  const state = domains.get(database)
  if (!state) return
  state.closed = true
  state.entries.clear()
}

export * as SessionScope from "./scope"
