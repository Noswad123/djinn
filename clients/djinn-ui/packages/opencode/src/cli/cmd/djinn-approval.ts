import { cmd } from "./cmd"

export const DjinnApprovalCommand = cmd({
  command: "djinn-approval",
  describe: false,
  builder: (yargs) =>
    yargs
      .option("request", {
        type: "string",
        demandOption: true,
      })
      .option("response", {
        type: "string",
        demandOption: true,
      }),
  async handler(args) {
    await runDjinnApprovalCommand(args.request, args.response)
  },
})

export async function runDjinnApprovalCommand(requestPath: string, responsePath: string) {
  const approval = await import("@opencode-ai/tui/djinn/permission-approval")
  const request = await Bun.file(requestPath).json()
  await Bun.write(responsePath, JSON.stringify(await approval.runDjinnPermissionApproval(request), null, 2) + "\n")
}
