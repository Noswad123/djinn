// Keep capability negotiation outside the application's import/initialization graph.
if (process.argv[2] === "capabilities") {
  if (process.argv.length > 4 || (process.argv.length === 4 && process.argv[3] !== "--json")) {
    process.stderr.write("Usage: djinn-ui capabilities [--json]\n")
    process.exitCode = 1
  } else {
    const { capabilities } = await import("./capabilities")
    process.stdout.write(JSON.stringify(capabilities()) + "\n")
  }
} else {
  await import("./main")
}
