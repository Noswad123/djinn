import { parseArgs } from "node:util"
import { EventMaintenance } from "../src/event/maintenance"

if (import.meta.main) {
  try {
    const { values } = parseArgs({
      args: Bun.argv.slice(2),
      strict: true,
      allowPositionals: false,
      options: {
        source: { type: "string" },
        output: { type: "string" },
        apply: { type: "boolean", default: false },
        stopped: { type: "boolean", default: false },
        sample: { type: "string" },
        "dry-run": { type: "boolean", default: false },
        help: { type: "boolean", default: false },
      },
    })
    if (values.help) {
      console.log(
        "bun script/compact-events.ts --source ABSOLUTE --output ABSOLUTE [--sample 1000] [--dry-run | --apply --stopped]",
      )
    } else {
      if (!values.source || !values.output) throw new Error("Explicit --source and --output are required")
      if (values.apply && values["dry-run"]) throw new Error("--apply and --dry-run are mutually exclusive")
      console.log(
        JSON.stringify(
          await EventMaintenance.compact({
            source: values.source,
            output: values.output,
            apply: values.apply,
            stopped: values.stopped,
            sample: values.sample === undefined ? undefined : Number(values.sample),
          }),
          null,
          2,
        ),
      )
    }
  } catch {
    // Do not echo arguments or SQLite errors: paths and constraints can contain secrets.
    console.error(
      "Compact-copy failed. Check explicit absolute paths, stopped-writer acknowledgement, available space, codec validity, and unused output/partial/lock names. Source was not replaced. See packages/core/docs/event-storage.md.",
    )
    process.exitCode = 1
  }
}
