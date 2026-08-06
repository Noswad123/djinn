const CHANNEL = "local"
const VERSION = "0.0.0-local"

export const Script = {
  get channel() {
    return CHANNEL
  },
  get version() {
    return VERSION
  },
  get preview() {
    return true
  },
  get release(): boolean {
    return false
  },
}
console.log(`opencode script`, JSON.stringify(Script, null, 2))
