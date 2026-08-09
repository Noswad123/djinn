export type DjinnSessionListRowInput = {
  name: string
  display_name?: string
  next_action?: string
}

export function djinnSessionListTitle(session: DjinnSessionListRowInput) {
  return session.display_name?.trim() || session.name
}

export function djinnSessionListActionSummary(nextAction: string) {
  const summary = nextAction.split(":", 1)[0]?.trim() || nextAction.trim()
  return summary.replaceAll("request.md", "request")
}

export function djinnSessionListActionLine(session: DjinnSessionListRowInput) {
  const nextAction = session.next_action?.trim()
  if (!nextAction) return undefined
  return `Action: ${djinnSessionListActionSummary(nextAction)}`
}

export * as DjinnSessionList from "./session-list"
