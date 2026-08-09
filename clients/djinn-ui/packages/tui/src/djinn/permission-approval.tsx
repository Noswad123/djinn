import { createCliRenderer, RGBA } from "@opentui/core"
import { render, useKeyboard, useTerminalDimensions } from "@opentui/solid"
import { createMemo, createSignal, For, Show } from "solid-js"
import { destroyRenderer } from "../util/renderer"

export type DjinnPermissionApprovalRequest = {
  action: string
  description: string
  metadata?: Record<string, unknown>
}

export type DjinnPermissionApprovalResponse = {
  decision: "allow" | "allow_session" | "deny"
  resources?: string[]
}

type PreviewLine = {
  kind: "title" | "meta" | "hunk" | "add" | "remove" | "context"
  text: string
}

const colors = {
  background: RGBA.fromInts(17, 17, 27, 255),
  element: RGBA.fromInts(49, 50, 68, 255),
  text: RGBA.fromInts(205, 214, 244, 255),
  muted: RGBA.fromInts(147, 153, 178, 255),
  warning: RGBA.fromInts(249, 226, 175, 255),
  add: RGBA.fromInts(166, 227, 161, 255),
  remove: RGBA.fromInts(243, 139, 168, 255),
  selected: RGBA.fromInts(137, 180, 250, 255),
}

export async function runDjinnPermissionApproval(request: DjinnPermissionApprovalRequest) {
  const renderer = await createCliRenderer({
    externalOutputMode: "passthrough",
    targetFps: 60,
    gatherStats: false,
    exitOnCtrlC: false,
    useKittyKeyboard: {},
    autoFocus: false,
    openConsoleOnError: false,
    useMouse: true,
  })
  let finish!: (decision: DjinnPermissionApprovalResponse) => void
  const done = new Promise<DjinnPermissionApprovalResponse>((resolve) => {
    finish = resolve
  })

  await render(() => <DjinnPermissionApproval request={request} onDecision={finish} />, renderer)
  const decision = await done
  if (!renderer.isDestroyed) destroyRenderer(renderer)
  return decision
}

function DjinnPermissionApproval(props: {
  request: DjinnPermissionApprovalRequest
  onDecision: (decision: DjinnPermissionApprovalResponse) => void
}) {
  const dimensions = useTerminalDimensions()
  const [selected, setSelected] = createSignal<"allow" | "allow_session" | "deny">("allow")
  const [scroll, setScroll] = createSignal(0)
  const lines = createMemo(() => djinnPermissionPreviewLines(props.request.metadata ?? {}))
  const maxBodyLines = createMemo(() => Math.max(6, dimensions().height - 12))
  const visibleLines = createMemo(() => lines().slice(scroll(), scroll() + maxBodyLines()))
  const scrollRange = createMemo(() => ({
    first: lines().length === 0 ? 0 : scroll() + 1,
    last: Math.min(lines().length, scroll() + maxBodyLines()),
    total: lines().length,
  }))
  const resources = createMemo(() => djinnPermissionResources(props.request.metadata ?? {}))

  function decide(decision: DjinnPermissionApprovalResponse["decision"]) {
    props.onDecision({ decision, resources: decision === "allow_session" ? resources() : undefined })
  }

  function move(direction: 1 | -1) {
    const options = ["allow", "allow_session", "deny"] as const
    const index = options.indexOf(selected())
    setSelected(options[(index + direction + options.length) % options.length])
  }

  useKeyboard((evt) => {
    if (evt.name === "escape" || evt.name === "n") {
      evt.preventDefault()
      decide("deny")
      return
    }
    if (evt.name === "y") {
      evt.preventDefault()
      decide("allow")
      return
    }
    if (evt.name === "s") {
      evt.preventDefault()
      decide("allow_session")
      return
    }
    if (evt.name === "return") {
      evt.preventDefault()
      decide(selected())
      return
    }
    if (evt.name === "left" || evt.name === "h") {
      evt.preventDefault()
      move(-1)
      return
    }
    if (evt.name === "right" || evt.name === "l") {
      evt.preventDefault()
      move(1)
      return
    }
    if (evt.name === "down" || evt.name === "j") {
      evt.preventDefault()
      setScroll((value) => Math.min(Math.max(0, lines().length - maxBodyLines()), value + 1))
      return
    }
    if (evt.name === "up" || evt.name === "k") {
      evt.preventDefault()
      setScroll((value) => Math.max(0, value - 1))
    }
  })

  return (
    <box
      flexDirection="column"
      flexGrow={1}
      minHeight={0}
      backgroundColor={colors.background}
      paddingLeft={2}
      paddingRight={2}
      paddingTop={1}
      gap={1}
    >
      <box flexDirection="column" flexShrink={0} gap={1}>
        <box flexDirection="row" gap={1}>
          <text fg={colors.warning}>△</text>
          <text fg={colors.text}>Djinn permission approval required</text>
        </box>
        <text fg={colors.muted}>{props.request.description}</text>
        <text fg={colors.muted}>Action: {props.request.action}</text>
      </box>
      <box
        flexDirection="column"
        flexGrow={1}
        minHeight={0}
        border
        borderStyle="rounded"
        borderColor={colors.warning}
        paddingLeft={1}
        paddingRight={1}
      >
        <For each={visibleLines()}>
          {(line) => (
            <text fg={previewLineColor(line)} wrapMode="none">
              {line.text}
            </text>
          )}
        </For>
        <Show when={lines().length === 0}>
          <text fg={colors.muted}>No structured preview was provided.</text>
        </Show>
      </box>
      <box flexDirection="row" gap={1} flexShrink={0}>
        <ApprovalOption id="allow" selected={selected()} label="Allow once" shortcut="y" onSelect={() => decide("allow")} />
        <ApprovalOption
          id="allow_session"
          selected={selected()}
          label="Allow session"
          shortcut="s"
          onSelect={() => decide("allow_session")}
        />
        <ApprovalOption id="deny" selected={selected()} label="Deny" shortcut="n/esc" onSelect={() => decide("deny")} />
      </box>
      <text fg={colors.muted} flexShrink={0}>
        ←/→ choose · enter confirm · j/k scroll {scrollRange().first}-{scrollRange().last} of {scrollRange().total}
      </text>
    </box>
  )
}

function ApprovalOption(props: {
  id: "allow" | "allow_session" | "deny"
  selected: "allow" | "allow_session" | "deny"
  label: string
  shortcut: string
  onSelect: () => void
}) {
  const active = () => props.selected === props.id
  return (
    <box
      paddingLeft={1}
      paddingRight={1}
      backgroundColor={active() ? colors.selected : colors.element}
      onMouseUp={props.onSelect}
    >
      <text fg={active() ? colors.background : colors.text}>
        {props.label} <span style={{ fg: active() ? colors.background : colors.muted }}>{props.shortcut}</span>
      </text>
    </box>
  )
}

function previewLineColor(line: PreviewLine) {
  if (line.kind === "add") return colors.add
  if (line.kind === "remove") return colors.remove
  if (line.kind === "hunk") return colors.warning
  if (line.kind === "meta") return colors.muted
  return colors.text
}

export function djinnPermissionPreviewLines(metadata: Record<string, unknown>) {
  const preview = Array.isArray(metadata.preview) ? metadata.preview : []
  if (preview.length > 0) return preview.flatMap((item) => previewItemLines(item))
  return metadataSummaryLines(metadata)
}

export function djinnPermissionResources(metadata: Record<string, unknown>) {
  const values: string[] = []
  const preview = Array.isArray(metadata.preview) ? metadata.preview : []
  for (const item of preview) {
    if (!isRecord(item)) continue
    pushUnique(values, stringField(item, "path"))
    pushUnique(values, stringField(item, "new_path"))
  }
  if (Array.isArray(metadata.resources)) {
    for (const item of metadata.resources) if (typeof item === "string") pushUnique(values, item)
  }
  if (typeof metadata.resource === "string") pushUnique(values, metadata.resource)
  return values
}

function previewItemLines(item: unknown): PreviewLine[] {
  if (!isRecord(item)) return []
  const operation = stringField(item, "operation") || "operation"
  const path = stringField(item, "relative_path") || stringField(item, "path") || "<unknown>"
  const added = numberField(item, "lines_added") ?? 0
  const removed = numberField(item, "lines_removed") ?? 0
  const lines: PreviewLine[] = [{ kind: "title", text: `${operation} ${path} (+${added}/-${removed})` }]
  const newPath = stringField(item, "relative_new_path") || stringField(item, "new_path")
  if (newPath) lines.push({ kind: "meta", text: `  -> ${newPath}` })
  const hunks = Array.isArray(item.hunks) ? item.hunks : []
  hunks.forEach((hunk, index) => {
    lines.push({ kind: "hunk", text: `  @@ hunk ${index + 1}` })
    if (!isRecord(hunk) || !Array.isArray(hunk.lines)) return
    for (const rawLine of hunk.lines) {
      if (!isRecord(rawLine)) continue
      const kind = stringField(rawLine, "kind")
      const content = stringField(rawLine, "content") ?? ""
      if (kind === "add") lines.push({ kind: "add", text: `  + ${content}` })
      else if (kind === "remove") lines.push({ kind: "remove", text: `  - ${content}` })
      else lines.push({ kind: "context", text: `    ${content}` })
    }
  })
  lines.push({ kind: "meta", text: "" })
  return lines
}

function metadataSummaryLines(metadata: Record<string, unknown>): PreviewLine[] {
  return Object.entries(metadata).map(([key, value]) => ({
    kind: "meta",
    text: `${key}: ${typeof value === "string" ? value : JSON.stringify(value)}`,
  }))
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null
}

function stringField(record: Record<string, unknown>, field: string) {
  const value = record[field]
  return typeof value === "string" && value.trim() ? value : undefined
}

function numberField(record: Record<string, unknown>, field: string) {
  const value = record[field]
  return typeof value === "number" ? value : undefined
}

function pushUnique(values: string[], value: string | undefined) {
  if (!value?.trim()) return
  if (values.includes(value)) return
  values.push(value)
}
