/** Keep renderer and backend discovery gates identical, including malformed flags. */
export function enabledFlag(value: string | undefined) {
  return boolean(value, true) ?? false
}

export function disabledFlag(value: string | undefined) {
  return boolean(value, false) ?? true
}

export function enabled(env: Record<string, string | undefined>) {
  return enabledFlag(env.DJINN_NUMEN) && !disabledFlag(env.DJINN_NUMEN_DISABLED)
}

function boolean(value: string | undefined, fallback: boolean) {
  if (value === undefined) return fallback
  // Match Effect Config.boolean's existing case-sensitive, untrimmed vocabulary.
  if (["true", "yes", "on", "1", "y"].includes(value)) return true
  if (["false", "no", "off", "0", "n"].includes(value)) return false
  return undefined
}

export * as NumenConfig from "./config"
