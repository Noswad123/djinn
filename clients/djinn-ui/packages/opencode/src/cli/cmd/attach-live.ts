import { cmd } from "./cmd"
import { LiveAttachClient } from "../tui/live-attach-client"

export const AttachLiveCommand = cmd({
  command: "attach-live",
  describe: "attach the full chat UI to an existing live Numen worker",
  builder: (yargs) =>
    yargs
      .option("instance", { type: "string", demandOption: true })
      .option("session", { type: "string", demandOption: true })
      .option("float", { type: "boolean", default: false })
      .option("generation", { type: "string", hidden: true })
      .option("incarnation", { type: "string", hidden: true }),
  handler: async (args) => {
    await LiveAttachClient.run(args)
  },
})
