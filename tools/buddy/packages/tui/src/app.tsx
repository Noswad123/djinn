import { render, TimeToFirstDraw, useRenderer, useTerminalDimensions } from "@opentui/solid"
import { registerOpencodeSpinner } from "./component/register-spinner"
import { createDefaultOpenTuiKeymap } from "@opentui/keymap/opentui"
import { Deferred, Effect } from "effect"
import { Global } from "@opencode-ai/core/global"
import { Flag } from "@opencode-ai/core/flag/flag"
import { InstallationVersion } from "@opencode-ai/core/installation/version"
import { ClipboardProvider, useClipboard } from "./context/clipboard"
import { ExitProvider, useExit } from "./context/exit"
import { EpilogueProvider } from "./context/epilogue"
import * as Selection from "./util/selection"
import { createCliRenderer, MouseButton } from "@opentui/core"
import { RouteProvider, useRoute } from "./context/route"
import {
  Switch,
  Match,
  createEffect,
  createMemo,
  ErrorBoundary,
  createSignal,
  onMount,
  onCleanup,
  batch,
  Show,
  on,
  For,
  createResource,
} from "solid-js"
import { TuiPathsProvider, TuiStartupProvider, TuiTerminalEnvironmentProvider, useTuiStartup } from "./context/runtime"
import { DialogProvider, useDialog } from "./ui/dialog"
import { DialogProvider as DialogProviderList } from "./component/dialog-provider"
import { ErrorComponent } from "./component/error-component"
import { PluginRouteMissing } from "./component/plugin-route-missing"
import { ProjectProvider, useProject } from "./context/project"
import { EditorContextProvider } from "./context/editor"
import { useEvent } from "./context/event"
import { SDKProvider, useSDK } from "./context/sdk"
import { StartupLoading } from "./component/startup-loading"
import { SyncProvider, useSync } from "./context/sync"
import { DataProvider } from "./context/data"
import { LocationProvider } from "./context/location"
import { LocalProvider, useLocal } from "./context/local"
import { PermissionProvider } from "./context/permission"
import { DialogModel } from "./component/dialog-model"
import { useConnected } from "./component/use-connected"
import { DialogMcp } from "./component/dialog-mcp"
import { DialogStatus } from "./component/dialog-status"
import { DialogDebug } from "./component/dialog-debug"
import { DialogThemeList } from "./component/dialog-theme-list"
import { DialogHelp } from "./ui/dialog-help"
import { DialogAgent } from "./component/dialog-agent"
import { DialogSessionList } from "./component/dialog-session-list"
import { DialogWorkspaceList } from "./component/dialog-workspace-list"
import { DialogConsoleOrg } from "./component/dialog-console-org"
import { ThemeProvider, useTheme } from "./context/theme"
import { Home } from "./routes/home"
import { Session } from "./routes/session"
import { PromptHistoryProvider } from "./component/prompt/history"
import { FrecencyProvider } from "./component/prompt/frecency"
import { PromptStashProvider } from "./component/prompt/stash"
import { ToastProvider, useToast } from "./ui/toast"
import { isDefaultTitle } from "./util/session"
import { KVProvider, useKV } from "./context/kv"
import * as Model from "./util/model"
import { ArgsProvider, useArgs, type Args } from "./context/args"
import open from "open"
import { PromptRefProvider, usePromptRef } from "./context/prompt"
import { TuiConfigProvider, useTuiConfig, type TuiConfig } from "./config"
import { createTuiApiAdapters } from "./plugin/adapters"
import { createTuiApi } from "./plugin/api"
import { createPluginRuntime, PluginRuntimeProvider, usePluginRuntime, type TuiPluginHost } from "./plugin/runtime"
import { CommandPaletteDialog } from "./component/command-palette"
import {
  COMMAND_PALETTE_COMMAND,
  OPENCODE_BASE_MODE,
  OpencodeKeymapProvider,
  registerOpencodeKeymap,
  useBindings,
  useOpencodeKeymap,
} from "./keymap"

import type { EventSource } from "./context/sdk"
import { DialogVariant } from "./component/dialog-variant"
import { createTuiAttention } from "./attention"
import * as TuiAudio from "./audio"
import { win32DisableProcessedInput, win32FlushInputBuffer } from "./terminal-win32"
import { destroyRenderer } from "./util/renderer"
import { cliErrorMessage, errorFormat } from "./util/error"

registerOpencodeSpinner()

const appGlobalBindingCommands = [
  "session.list",
  "session.new",
  "session.quick_switch.1",
  "session.quick_switch.2",
  "session.quick_switch.3",
  "session.quick_switch.4",
  "session.quick_switch.5",
  "session.quick_switch.6",
  "session.quick_switch.7",
  "session.quick_switch.8",
  "session.quick_switch.9",
] as const

const appBindingCommands = [
  "tab.next",
  "tab.previous",
  "command.palette.show",
  "model.list",
  "model.cycle_recent",
  "model.cycle_recent_reverse",
  "model.cycle_favorite",
  "model.cycle_favorite_reverse",
  "agent.list",
  "mcp.list",
  "agent.cycle",
  "agent.cycle.reverse",
  "variant.cycle",
  "variant.list",
  "provider.connect",
  "console.org.switch",
  "opencode.status",
  "opencode.debug",
  "theme.switch",
  "theme.switch_mode",
  "theme.mode.lock",
  "help.show",
  "docs.open",
  "diff.open",
  "workspace.list",
  "app.debug",
  "app.console",
  "app.heap_snapshot",
  "terminal.suspend",
  "terminal.title.toggle",
  "app.toggle.animations",
  "app.toggle.file_context",
  "app.toggle.diffwrap",
  "app.toggle.paste_summary",
  "app.toggle.session_directory_filter",
] as const

type AppTabID = "chat" | "sessions" | "memories" | "suggestions" | "skills" | "tools"

const buddyTabs: { id: AppTabID; title: string }[] = [
  { id: "chat", title: "Chat" },
  { id: "sessions", title: "Sessions" },
  { id: "memories", title: "Memories" },
  { id: "suggestions", title: "Suggestions" },
  { id: "skills", title: "Skills" },
  { id: "tools", title: "Tools" },
]

const djinnDashboardTabNotes: Record<Exclude<AppTabID, "chat" | "sessions">, string> = {
  memories: "Memory review will move here after Buddy has a Djinn data bridge for memories.",
  suggestions: "Suggestion triage will move here after Buddy has a Djinn data bridge for suggestions.",
  skills: "Skill browsing will move here after Buddy has a Djinn data bridge for skills.",
  tools: "Tool browsing will move here after Buddy has a Djinn data bridge for tool indexes.",
}

export type TuiInput = {
  url: string
  args: Args
  config: TuiConfig.Resolved
  onSnapshot?: () => Promise<string[]>
  directory?: string
  fetch?: typeof fetch
  headers?: RequestInit["headers"]
  events?: EventSource
  pluginHost: TuiPluginHost
}

function errorMessage(error: unknown) {
  if (
    typeof error === "object" &&
    error !== null &&
    "data" in error &&
    typeof error.data === "object" &&
    error.data !== null &&
    "message" in error.data &&
    typeof error.data.message === "string"
  ) {
    return error.data.message
  }
  return error instanceof Error ? error.message : String(error)
}

export const run = Effect.fn("Tui.run")(function* (input: TuiInput) {
  const global = yield* Global.Service
  const exit = { epilogue: undefined as string | undefined, reason: undefined as unknown }
  const result = yield* Effect.scoped(
    Effect.gen(function* () {
      const renderer = yield* Effect.acquireRelease(
        Effect.tryPromise({
          try: () =>
            createCliRenderer({
              externalOutputMode: "passthrough",
              targetFps: 60,
              gatherStats: false,
              exitOnCtrlC: false,
              useKittyKeyboard: {},
              autoFocus: false,
              openConsoleOnError: false,
              useMouse: !Flag.OPENCODE_DISABLE_MOUSE && input.config.mouse,
              consoleOptions: {
                keyBindings: [{ name: "y", ctrl: true, action: "copy-selection" }],
              },
            }),
          catch: (error) => (error instanceof Error ? error : new Error(String(error))),
        }),
        (renderer) =>
          Effect.sync(() => {
            destroyRenderer(renderer)
          }),
      )
      win32DisableProcessedInput()
      const keymap = createDefaultOpenTuiKeymap(renderer)
      yield* Effect.acquireRelease(
        Effect.sync(() => registerOpencodeKeymap(keymap, renderer, input.config)),
        (unregister) => Effect.sync(unregister),
      )
      yield* Effect.addFinalizer(() =>
        Effect.promise(async () => {
          try {
            await input.pluginHost.dispose()
          } catch (error) {
            console.error("Failed to dispose TUI plugins", error)
          }
        }),
      )
      yield* Effect.addFinalizer(() => Effect.sync(TuiAudio.dispose))
      const shutdown = yield* Deferred.make<unknown>()
      const onSighup = () => destroyRenderer(renderer)
      yield* Effect.acquireRelease(
        Effect.sync(() => process.on("SIGHUP", onSighup)),
        () => Effect.sync(() => process.off("SIGHUP", onSighup)),
      )
      renderer.once("destroy", () => Deferred.doneUnsafe(shutdown, Effect.void))
      const pluginRuntime = createPluginRuntime()

      yield* Effect.tryPromise(async () => {
        // Prewarm palette before ThemeProvider mounts so `system` theme avoids a first-paint fallback flash.
        void renderer.getPalette({ size: 16 }).catch(() => undefined)
        const mode = (await renderer.waitForThemeMode(1000)) ?? "dark"
        if (renderer.isDestroyed) return

        await render(() => {
          return (
            <ExitProvider
              exit={(reason) => {
                if (renderer.isDestroyed) return
                exit.reason = reason
                destroyRenderer(renderer)
              }}
            >
              <EpilogueProvider set={(value) => (exit.epilogue = value)}>
                <ErrorBoundary fallback={(error, reset) => <ErrorComponent error={error} reset={reset} mode={mode} />}>
                  <TuiPathsProvider
                    value={{
                      cwd: process.cwd(),
                      home: global.home,
                      state: global.state,
                      worktree: global.data + "/worktree",
                    }}
                  >
                    <TuiTerminalEnvironmentProvider
                      value={{
                        platform: process.platform,
                        multiplexer: process.env.TMUX ? "tmux" : process.env.STY ? "screen" : undefined,
                        displayServer: process.env.WAYLAND_DISPLAY
                          ? "wayland"
                          : process.env.DISPLAY
                            ? "x11"
                            : undefined,
                      }}
                    >
                      <TuiStartupProvider
                        value={{
                          initialRoute: process.env.OPENCODE_ROUTE ? JSON.parse(process.env.OPENCODE_ROUTE) : undefined,
                          skipInitialLoading: Boolean(process.env.OPENCODE_FAST_BOOT),
                        }}
                      >
                        <ClipboardProvider>
                          <OpencodeKeymapProvider keymap={keymap}>
                            <ArgsProvider {...input.args}>
                              <KVProvider>
                                <ToastProvider>
                                  <RouteProvider
                                    initialRoute={
                                      input.args.continue
                                        ? {
                                            type: "session",
                                            sessionID: "dummy",
                                          }
                                        : undefined
                                    }
                                  >
                                    <TuiConfigProvider config={input.config}>
                                      <PluginRuntimeProvider value={pluginRuntime}>
                                        <SDKProvider
                                          url={input.url}
                                          directory={input.directory}
                                          fetch={input.fetch}
                                          headers={input.headers}
                                          events={input.events}
                                        >
                                          <PermissionProvider>
                                            <ProjectProvider>
                                              <SyncProvider>
                                                <DataProvider>
                                                  <ThemeProvider mode={mode}>
                                                    <LocalProvider>
                                                      <PromptStashProvider>
                                                        <DialogProvider>
                                                          <FrecencyProvider>
                                                            <PromptHistoryProvider>
                                                              <PromptRefProvider>
                                                                <EditorContextProvider>
                                                                  <LocationProvider>
                                                                    <App
                                                                      onSnapshot={input.onSnapshot}
                                                                      pluginHost={input.pluginHost}
                                                                    />
                                                                  </LocationProvider>
                                                                </EditorContextProvider>
                                                              </PromptRefProvider>
                                                            </PromptHistoryProvider>
                                                          </FrecencyProvider>
                                                        </DialogProvider>
                                                      </PromptStashProvider>
                                                    </LocalProvider>
                                                  </ThemeProvider>
                                                </DataProvider>
                                              </SyncProvider>
                                            </ProjectProvider>
                                          </PermissionProvider>
                                        </SDKProvider>
                                      </PluginRuntimeProvider>
                                    </TuiConfigProvider>
                                  </RouteProvider>
                                </ToastProvider>
                              </KVProvider>
                            </ArgsProvider>
                          </OpencodeKeymapProvider>
                        </ClipboardProvider>
                      </TuiStartupProvider>
                    </TuiTerminalEnvironmentProvider>
                  </TuiPathsProvider>
                </ErrorBoundary>
              </EpilogueProvider>
            </ExitProvider>
          )
        }, renderer)
      })
      yield* Deferred.await(shutdown)
      return { epilogue: exit.epilogue, reason: exit.reason }
    }),
  )
  yield* Effect.sync(() => {
    win32FlushInputBuffer()
    if (result.reason !== undefined)
      process.stderr.write((cliErrorMessage(result.reason) ?? errorFormat(result.reason)) + "\n")
    if (result.epilogue) process.stdout.write(result.epilogue + "\n")
  })
})

function App(props: { onSnapshot?: () => Promise<string[]>; pluginHost: TuiPluginHost }) {
  const startup = useTuiStartup()
  const tuiConfig = useTuiConfig()
  const route = useRoute()
  const dimensions = useTerminalDimensions()
  const renderer = useRenderer()
  const dialog = useDialog()
  const local = useLocal()
  const kv = useKV()
  const keymap = useOpencodeKeymap()
  const event = useEvent()
  const sdk = useSDK()
  const toast = useToast()
  const themeState = useTheme()
  const { theme, mode, setMode, locked, lock, unlock } = themeState
  const sync = useSync()
  const project = useProject()
  const exit = useExit()
  const promptRef = usePromptRef()
  const pluginRuntime = usePluginRuntime()
  const attention = createTuiAttention({ renderer, config: tuiConfig, kv })
  const clipboard = useClipboard()

  const api = createTuiApi(
    createTuiApiAdapters({
      version: InstallationVersion,
      tuiConfig,
      dialog,
      keymap,
      kv,
      route,
      routes: pluginRuntime.routes,
      event,
      sdk,
      sync,
      theme: themeState,
      toast,
      renderer,
      attention,
      Slot: pluginRuntime.Slot,
    }),
  )
  const [ready, setReady] = createSignal(false)
  props.pluginHost
    .start({
      api,
      config: tuiConfig,
      runtime: pluginRuntime,
      dispose: () => attention.dispose(),
    })
    .catch((error) => {
      console.error("Failed to load TUI plugins", error)
    })
    .finally(() => {
      setReady(true)
    })

  // Let selection copy/dismiss win ahead of normal bindings when explicit copy is required.
  const offSelectionKeys = keymap.intercept(
    "key",
    ({ event }) => {
      if (!Flag.OPENCODE_EXPERIMENTAL_DISABLE_COPY_ON_SELECT) return
      Selection.handleSelectionKey(renderer, toast, event, clipboard)
    },
    { priority: 1 },
  )
  onCleanup(() => {
    offSelectionKeys()
    attention.dispose()
  })

  // Wire up console copy-to-clipboard via opentui's onCopySelection callback
  renderer.console.onCopySelection = async (text: string) => {
    if (!text || text.length === 0) return

    await clipboard
      .write?.(text)
      .then(() => toast.show({ message: "Copied to clipboard", variant: "info" }))
      .catch(toast.error)

    renderer.clearSelection()
  }
  const [terminalTitleEnabled, setTerminalTitleEnabled] = createSignal(kv.get("terminal_title_enabled", true))
  const [pasteSummaryEnabled, setPasteSummaryEnabled] = createSignal(
    kv.get("paste_summary_enabled", !sync.data.config.experimental?.disable_paste_summary),
  )
  const [activeTab, setActiveTab] = createSignal<AppTabID>("chat")
  const tabs = createMemo(() => buddyTabs)

  const activeTabIndex = createMemo(() => Math.max(0, tabs().findIndex((tab) => tab.id === activeTab())))

  function moveTab(direction: number) {
    if (dialog.stack.length > 0) return
    const items = tabs()
    const next = (activeTabIndex() + direction + items.length) % items.length
    setActiveTab(items[next]?.id ?? "chat")
  }

  createEffect(
    on(
      () => (route.data.type === "session" ? route.data.sessionID : route.data.type),
      () => setActiveTab("chat"),
    ),
  )

  // Update terminal window title based on current route and session
  createEffect(() => {
    if (!terminalTitleEnabled() || Flag.OPENCODE_DISABLE_TERMINAL_TITLE) return

    if (route.data.type === "home") {
      renderer.setTerminalTitle("OpenCode")
      return
    }

    if (route.data.type === "session") {
      const session = sync.session.get(route.data.sessionID)
      if (!session || isDefaultTitle(session.title)) {
        renderer.setTerminalTitle("OpenCode")
        return
      }

      const title = session.title.length > 40 ? session.title.slice(0, 37) + "..." : session.title
      renderer.setTerminalTitle(`OC | ${title}`)
      return
    }

    if (route.data.type === "plugin") {
      renderer.setTerminalTitle(`OC | ${route.data.id}`)
    }
  })

  const args = useArgs()
  onMount(() => {
    batch(() => {
      if (args.agent) local.agent.set(args.agent)
      if (args.model) {
        const { providerID, modelID } = Model.parse(args.model)
        if (!providerID || !modelID)
          return toast.show({
            variant: "warning",
            message: `Invalid model format: ${args.model}`,
            duration: 3000,
          })
        local.model.set({ providerID, modelID }, { recent: true })
      }
      if (args.sessionID && !args.fork) {
        route.navigate({
          type: "session",
          sessionID: args.sessionID,
        })
      }
    })
  })

  let continued = false
  createEffect(() => {
    // When using -c, session list is loaded in blocking phase, so we can navigate at "partial"
    if (continued || sync.status === "loading" || !args.continue) return
    const match = sync.data.session
      .toSorted((a, b) => b.time.updated - a.time.updated)
      .find((x) => x.parentID === undefined)?.id
    if (match) {
      continued = true
      if (args.fork) {
        void sdk.client.session.fork({ sessionID: match }).then((result) => {
          if (result.data?.id) {
            route.navigate({ type: "session", sessionID: result.data.id })
          } else {
            toast.show({ message: "Failed to fork session", variant: "error" })
          }
        })
      } else {
        route.navigate({ type: "session", sessionID: match })
      }
    }
  })

  // Handle --session with --fork: wait for sync to be fully complete before forking
  // (session list loads in non-blocking phase for --session, so we must wait for "complete"
  // to avoid a race where reconcile overwrites the newly forked session)
  let forked = false
  createEffect(() => {
    if (forked || sync.status !== "complete" || !args.sessionID || !args.fork) return
    forked = true
    void sdk.client.session.fork({ sessionID: args.sessionID }).then((result) => {
      if (result.data?.id) {
        route.navigate({ type: "session", sessionID: result.data.id })
      } else {
        toast.show({ message: "Failed to fork session", variant: "error" })
      }
    })
  })

  createEffect(
    on(
      () => sync.status === "complete" && sync.data.provider.length === 0,
      (isEmpty, wasEmpty) => {
        // only trigger when we transition into an empty-provider state
        if (!isEmpty || wasEmpty) return
        dialog.replace(() => <DialogProviderList />)
      },
    ),
  )

  const connected = useConnected()
  const currentWorktreeWorkspace = createMemo(() => {
    const workspaceID = project.workspace.current()
    if (!workspaceID) return
    const workspace = project.workspace.get(workspaceID)
    if (workspace?.type !== "worktree" || !workspace.directory) return
    return workspace
  })
  const appCommands = createMemo(() =>
    [
      {
        name: "tab.next",
        title: "Next tab",
        category: "Navigation",
        hidden: true,
        run: () => {
          moveTab(1)
        },
      },
      {
        name: "tab.previous",
        title: "Previous tab",
        category: "Navigation",
        hidden: true,
        run: () => {
          moveTab(-1)
        },
      },
      {
        name: COMMAND_PALETTE_COMMAND,
        title: "Show command palette",
        category: "System",
        hidden: true,
        run: () => {
          dialog.replace(() => <CommandPaletteDialog />)
        },
      },
      {
        name: "session.list",
        title: "Switch session",
        category: "Session",
        suggested: sync.data.session.length > 0,
        slashName: "sessions",
        slashAliases: ["resume", "continue"],
        run: () => {
          dialog.replace(() => <DialogSessionList />)
        },
      },
      {
        name: "session.new",
        title: "New session",
        suggested: route.data.type === "session",
        category: "Session",
        slashName: "new",
        slashAliases: ["clear"],
        run: () => {
          route.navigate({
            type: "home",
          })
          dialog.clear()
        },
      },
      {
        name: "workspace.copy_path",
        title: "Copy worktree path",
        category: "Workspace",
        enabled: () => currentWorktreeWorkspace() !== undefined,
        run: async () => {
          const workspace = currentWorktreeWorkspace()
          if (!workspace?.directory) return
          await clipboard
            .write?.(workspace.directory)
            .then(() => toast.show({ message: "Copied worktree path", variant: "info" }))
            .catch(toast.error)
          dialog.clear()
        },
      },
      {
        name: "workspace.list",
        title: "Manage workspaces",
        category: "Workspace",
        hidden: !Flag.OPENCODE_EXPERIMENTAL_WORKSPACES,
        slashName: "workspaces",
        run: () => {
          dialog.replace(() => <DialogWorkspaceList />)
        },
      },
      ...Array.from({ length: 9 }, (_, i) => ({
        name: `session.quick_switch.${i + 1}`,
        title: `Switch to session in quick slot ${i + 1}`,
        category: "Session",
        hidden: true,
        run: () => {
          local.session.quickSwitch(i + 1)
        },
      })),
      {
        name: "model.list",
        title: "Switch model",
        suggested: true,
        category: "Agent",
        slashName: "models",
        // Bias /mo toward /models over /move without changing global fuzzy scoring.
        slashAliases: ["mo"],
        run: () => {
          dialog.replace(() => <DialogModel />)
        },
      },
      {
        name: "model.cycle_recent",
        title: "Model cycle",
        category: "Agent",
        hidden: true,
        run: () => {
          local.model.cycle(1)
        },
      },
      {
        name: "model.cycle_recent_reverse",
        title: "Model cycle reverse",
        category: "Agent",
        hidden: true,
        run: () => {
          local.model.cycle(-1)
        },
      },
      {
        name: "model.cycle_favorite",
        title: "Favorite cycle",
        category: "Agent",
        hidden: true,
        run: () => {
          local.model.cycleFavorite(1)
        },
      },
      {
        name: "model.cycle_favorite_reverse",
        title: "Favorite cycle reverse",
        category: "Agent",
        hidden: true,
        run: () => {
          local.model.cycleFavorite(-1)
        },
      },
      {
        name: "agent.list",
        title: "Switch agent",
        category: "Agent",
        slashName: "agents",
        run: () => {
          dialog.replace(() => <DialogAgent />)
        },
      },
      {
        name: "mcp.list",
        title: "Toggle MCPs",
        category: "Agent",
        slashName: "mcps",
        run: () => {
          dialog.replace(() => <DialogMcp />)
        },
      },
      {
        name: "agent.cycle",
        title: "Agent cycle",
        category: "Agent",
        hidden: true,
        run: () => {
          local.agent.move(1)
        },
      },
      {
        name: "variant.cycle",
        title: "Variant cycle",
        category: "Agent",
        run: () => {
          local.model.variant.cycle()
        },
      },
      {
        name: "variant.list",
        title: "Switch model variant",
        category: "Agent",
        hidden: local.model.variant.list().length === 0,
        slashName: "variants",
        run: () => {
          if (local.model.variant.list().length === 0) {
            return toast.show({
              title: "No variants available",
              message: "The current model does not support any variants.",
              variant: "info",
            })
          }
          dialog.replace(() => <DialogVariant />)
        },
      },
      {
        name: "agent.cycle.reverse",
        title: "Agent cycle reverse",
        category: "Agent",
        hidden: true,
        run: () => {
          local.agent.move(-1)
        },
      },
      {
        name: "provider.connect",
        title: "Connect provider",
        suggested: !connected(),
        slashName: "connect",
        run: () => {
          dialog.replace(() => <DialogProviderList />)
        },
        category: "Provider",
      },
      ...(sync.data.console_state.switchableOrgCount > 1
        ? [
            {
              name: "console.org.switch",
              title: "Switch org",
              suggested: Boolean(sync.data.console_state.activeOrgName),
              slashName: "org",
              slashAliases: ["orgs", "switch-org"],
              run: () => {
                dialog.replace(() => <DialogConsoleOrg />)
              },
              category: "Provider",
            },
          ]
        : []),
      {
        name: "opencode.status",
        title: "View status",
        slashName: "status",
        run: () => {
          dialog.replace(() => <DialogStatus />)
        },
        category: "System",
      },
      {
        name: "opencode.debug",
        title: "View debug info",
        slashName: "debug",
        run: () => {
          dialog.replace(() => <DialogDebug />)
        },
        category: "System",
      },
      {
        name: "theme.switch",
        title: "Switch theme",
        slashName: "themes",
        run: () => {
          dialog.replace(() => <DialogThemeList />)
        },
        category: "System",
      },
      {
        name: "theme.switch_mode",
        title: mode() === "dark" ? "Switch to light mode" : "Switch to dark mode",
        run: () => {
          setMode(mode() === "dark" ? "light" : "dark")
          dialog.clear()
        },
        category: "System",
      },
      {
        name: "theme.mode.lock",
        title: locked() ? "Unlock theme mode" : "Lock theme mode",
        run: () => {
          if (locked()) unlock()
          else lock()
          dialog.clear()
        },
        category: "System",
      },
      {
        name: "help.show",
        title: "Help",
        slashName: "help",
        run: () => {
          dialog.replace(() => <DialogHelp />)
        },
        category: "System",
      },
      {
        name: "docs.open",
        title: "Open docs",
        run: () => {
          open("https://opencode.ai/docs").catch(() => {})
          dialog.clear()
        },
        category: "System",
      },
      {
        name: "app.exit",
        title: "Exit the app",
        slashName: "exit",
        slashAliases: ["quit", "q"],
        run: () => exit(),
        category: "System",
      },
      {
        name: "app.debug",
        title: "Toggle debug panel",
        category: "System",
        run: () => {
          renderer.toggleDebugOverlay()
          dialog.clear()
        },
      },
      {
        name: "app.console",
        title: "Toggle console",
        category: "System",
        run: () => {
          renderer.console.toggle()
          dialog.clear()
        },
      },
      {
        name: "app.heap_snapshot",
        title: "Write heap snapshot",
        category: "System",
        run: async () => {
          const files = await props.onSnapshot?.()
          toast.show({
            variant: "info",
            message: `Heap snapshot written to ${files?.join(", ")}`,
            duration: 5000,
          })
          dialog.clear()
        },
      },
      {
        name: "terminal.suspend",
        title: "Suspend terminal",
        category: "System",
        hidden: true,
        enabled: process.platform !== "win32",
        run: () => {
          renderer.suspend()
          process.once("SIGCONT", () => renderer.resume())
          process.kill(0, "SIGTSTP")
        },
      },
      {
        name: "terminal.title.toggle",
        title: terminalTitleEnabled() ? "Disable terminal title" : "Enable terminal title",
        category: "System",
        run: () => {
          setTerminalTitleEnabled((prev) => {
            const next = !prev
            kv.set("terminal_title_enabled", next)
            if (!next) renderer.setTerminalTitle("")
            return next
          })
          dialog.clear()
        },
      },
      {
        name: "app.toggle.animations",
        title: kv.get("animations_enabled", true) ? "Disable animations" : "Enable animations",
        category: "System",
        run: () => {
          kv.set("animations_enabled", !kv.get("animations_enabled", true))
          dialog.clear()
        },
      },
      {
        name: "app.toggle.file_context",
        title: kv.get("file_context_enabled", true) ? "Disable file context" : "Enable file context",
        category: "System",
        run: () => {
          kv.set("file_context_enabled", !kv.get("file_context_enabled", true))
          dialog.clear()
        },
      },
      {
        name: "app.toggle.diffwrap",
        title: kv.get("diff_wrap_mode", "word") === "word" ? "Disable diff wrapping" : "Enable diff wrapping",
        category: "System",
        run: () => {
          const current = kv.get("diff_wrap_mode", "word")
          kv.set("diff_wrap_mode", current === "word" ? "none" : "word")
          dialog.clear()
        },
      },
      {
        name: "app.toggle.paste_summary",
        title: pasteSummaryEnabled() ? "Disable paste summary" : "Enable paste summary",
        category: "System",
        run: () => {
          setPasteSummaryEnabled((prev) => {
            const next = !prev
            kv.set("paste_summary_enabled", next)
            return next
          })
          dialog.clear()
        },
      },
      {
        name: "app.toggle.session_directory_filter",
        title: kv.get("session_directory_filter_enabled", true)
          ? "Disable session directory filtering"
          : "Enable session directory filtering",
        category: "System",
        run: async () => {
          kv.set("session_directory_filter_enabled", !kv.get("session_directory_filter_enabled", true))
          await sync.session.refresh()
          dialog.clear()
        },
      },
      {
        name: "permission.mode",
        title:
          local.permission.mode === "auto" ? "Disable auto-approve permissions" : "Enable auto-approve permissions",
        category: "System",
        run: () => {
          local.permission.toggle()
          dialog.clear()
        },
      },
    ].map((command) => ({
      namespace: "palette",
      ...command,
    })),
  )

  useBindings(() => ({
    commands: appCommands(),
  }))

  useBindings(() => ({
    mode: OPENCODE_BASE_MODE,
    bindings: tuiConfig.keybinds.gather("app", appBindingCommands),
  }))

  useBindings(() => ({
    bindings: tuiConfig.keybinds.gather("app.global", appGlobalBindingCommands),
  }))

  useBindings(() => ({
    mode: OPENCODE_BASE_MODE,
    enabled: () => {
      const current = promptRef.current
      if (!current?.focused) return true
      return current.current.input === ""
    },
    bindings: tuiConfig.keybinds.gather("app_exit", ["app.exit"]),
  }))

  event.on("tui.command.execute", (evt, { workspace }) => {
    if (workspace !== project.workspace.current()) return
    keymap.dispatchCommand(evt.properties.command)
  })

  event.on("tui.toast.show", (evt, { workspace }) => {
    if (workspace !== project.workspace.current()) return
    toast.show({
      title: evt.properties.title,
      message: evt.properties.message,
      variant: evt.properties.variant,
      duration: evt.properties.duration,
    })
  })

  event.on("tui.session.select", (evt, { workspace }) => {
    if (workspace !== project.workspace.current()) return
    route.navigate({
      type: "session",
      sessionID: evt.properties.sessionID,
    })
  })

  event.on("session.deleted", (evt) => {
    if (route.data.type === "session" && route.data.sessionID === evt.properties.info.id) {
      route.navigate({ type: "home" })
      toast.show({
        variant: "info",
        message: "The current session was deleted",
      })
    }
  })

  event.on("session.error", (evt, { workspace }) => {
    if (workspace !== project.workspace.current()) return
    const error = evt.properties.error
    if (error && typeof error === "object" && error.name === "MessageAbortedError") return
    const message = errorMessage(error)

    toast.show({
      variant: "error",
      message,
      duration: 5000,
    })
  })

  const plugin = createMemo(() => {
    if (!ready()) return
    if (route.data.type !== "plugin") return
    const render = pluginRuntime.routes.get(route.data.id)
    if (!render) return <PluginRouteMissing id={route.data.id} onHome={() => route.navigate({ type: "home" })} />
    return render({ params: route.data.data })
  })

  return (
    <box
      width={dimensions().width}
      height={dimensions().height}
      flexDirection="column"
      backgroundColor={theme.background}
      onMouseDown={(evt) => {
        if (!Flag.OPENCODE_EXPERIMENTAL_DISABLE_COPY_ON_SELECT) return
        if (evt.button !== MouseButton.RIGHT) return

        if (!Selection.copy(renderer, toast, clipboard)) return
        evt.preventDefault()
        evt.stopPropagation()
      }}
      onMouseUp={
        !Flag.OPENCODE_EXPERIMENTAL_DISABLE_COPY_ON_SELECT
          ? () => Selection.copy(renderer, toast, clipboard)
          : undefined
      }
    >
      <Show when={Flag.OPENCODE_SHOW_TTFD}>
        <TimeToFirstDraw />
      </Show>
      <Show when={ready()}>
        <BuddyTabBar tabs={tabs()} active={activeTab()} onSelect={setActiveTab} />
        <box flexGrow={1} minHeight={0} flexDirection="column">
          <Switch>
            <Match when={activeTab() === "chat"}>
              <Switch>
                <Match when={route.data.type === "home"}>
                  <Home />
                </Match>
                <Match when={route.data.type === "session"}>
                  <Show when={route.data.type === "session" ? route.data.sessionID : undefined} keyed>
                    {(_) => <Session />}
                  </Show>
                </Match>
              </Switch>
              {plugin()}
            </Match>
            <Match when={activeTab() === "sessions"}>
              <SessionTab
                onOpenBuddySession={(sessionID) => {
                  route.navigate({ type: "session", sessionID })
                  setActiveTab("chat")
                }}
              />
            </Match>
            <Match when={activeTab() === "memories"}>
              <DashboardPlaceholderTab tab="memories" />
            </Match>
            <Match when={activeTab() === "suggestions"}>
              <DashboardPlaceholderTab tab="suggestions" />
            </Match>
            <Match when={activeTab() === "skills"}>
              <DashboardPlaceholderTab tab="skills" />
            </Match>
            <Match when={activeTab() === "tools"}>
              <DashboardPlaceholderTab tab="tools" />
            </Match>
          </Switch>
        </box>
        <box flexShrink={0}>
          <pluginRuntime.Slot name="app_bottom" />
        </box>
        <pluginRuntime.Slot name="app" />
      </Show>
      <Show when={!startup.skipInitialLoading}>
        <StartupLoading ready={ready} />
      </Show>
    </box>
  )
}

function BuddyTabBar(props: {
  tabs: readonly { id: AppTabID; title: string; badge?: string }[]
  active: AppTabID
  onSelect: (tab: AppTabID) => void
}) {
  const { theme } = useTheme()

  return (
    <box
      flexDirection="row"
      flexShrink={0}
      paddingLeft={1}
      paddingRight={2}
      backgroundColor={theme.backgroundPanel}
    >
      <box flexDirection="row" gap={1}>
        <For each={props.tabs}>
          {(tab) => {
            const active = () => tab.id === props.active
            return (
              <box
                flexDirection="row"
                gap={1}
                paddingLeft={1}
                paddingRight={1}
                backgroundColor={active() ? theme.backgroundElement : theme.backgroundPanel}
                onMouseDown={() => props.onSelect(tab.id)}
              >
                <text fg={active() ? theme.primary : theme.textMuted}>{tab.title}</text>
                <Show when={tab.badge}>
                  {(badge) => <text fg={active() ? theme.text : theme.textMuted}>{badge()}</text>}
                </Show>
              </box>
            )
          }}
        </For>
      </box>
    </box>
  )
}

function SessionTab(props: { onOpenBuddySession: (sessionID: string) => void }) {
  const { theme } = useTheme()
  const toast = useToast()
  const [filter, setFilter] = createSignal("")
  const [selectedPath, setSelectedPath] = createSignal<string>()
  const [checked, setChecked] = createSignal(new Set<string>())
  const [status, setStatus] = createSignal("Load Djinn folder sessions with djinn session ls --json.")
  const [report, { refetch }] = createResource(loadDjinnSessionReport)
  const allSessions = createMemo(() => report()?.sessions ?? [])
  const query = createMemo(() => filter().trim().toLowerCase())
  const filteredSessions = createMemo(() =>
    allSessions()
      .filter((session) => {
        const q = query()
        if (!q) return true
        return [
          session.display_name,
          session.reference_name,
          session.path,
          session.lifecycle.state,
          session.lifecycle.mode,
          session.repo_path,
          session.workspace,
          session.summary_preview,
          session.next_action,
        ].some((value) => value?.toLowerCase().includes(q))
      })
      .toSorted(compareDjinnSessions),
  )
  const selectedSession = createMemo(() =>
    allSessions().find((session) => session.path === selectedPath()) ?? filteredSessions()[0],
  )
  const selectedSessionPaths = createMemo(() => {
    const values = Array.from(checked())
    if (values.length > 0) return values
    const session = selectedSession()
    return session ? [session.path] : []
  })

  createEffect(() => {
    const first = filteredSessions()[0]
    if (!first) {
      setSelectedPath()
      return
    }
    if (!selectedPath() || !filteredSessions().some((session) => session.path === selectedPath())) setSelectedPath(first.path)
  })

  function toggleSession(path: string) {
    setChecked((current) => {
      const next = new Set(current)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  function toggleAll() {
    const paths = filteredSessions().map((session) => session.path)
    if (paths.length === 0) return
    setChecked((current) => {
      if (paths.every((path) => current.has(path))) return new Set<string>()
      return new Set(paths)
    })
  }

  async function runAction(label: string, args: string[]) {
    setStatus(`${label}: running djinn ${args.join(" ")}`)
    await runDjinn(args)
      .then((output) => {
        setStatus(output.trim() || `${label}: complete`)
        toast.show({ message: `${label} complete`, variant: "success" })
      })
      .catch((error) => {
        const message = error instanceof Error ? error.message : String(error)
        setStatus(`${label}: ${message}`)
        toast.show({ title: label, message, variant: "error", duration: 5000 })
      })
  }

  async function runAndRefresh(label: string, args: string[]) {
    await runAction(label, args)
    await refetch()
  }

  function promote(kind: "memory" | "todo" | "skill" | "pattern") {
    const paths = selectedSessionPaths()
    if (paths.length === 0) {
      toast.show({ message: "Select at least one session to promote", variant: "warning" })
      return
    }
    void runAndRefresh(`Promote to ${kind}`, ["session", "promote", ...paths, "--type", kind])
  }

  const candidate = createMemo(() => selectedSession()?.candidates?.entries?.[0])

  return (
    <box flexGrow={1} minHeight={0} paddingLeft={2} paddingRight={2} paddingTop={1} gap={1}>
      <box flexDirection="row" justifyContent="space-between" flexShrink={0}>
        <box gap={1}>
          <text fg={theme.text}>Sessions</text>
          <text fg={theme.textMuted}>Folder-backed Djinn sessions, promotion selection, and focused actions.</text>
        </box>
        <box flexDirection="row" gap={2}>
          <text fg={theme.text} onMouseDown={() => void refetch()}>
            refresh
          </text>
          <text fg={theme.text} onMouseDown={toggleAll}>
            toggle all
          </text>
        </box>
      </box>
      <box flexDirection="row" gap={1} flexShrink={0}>
        <text fg={theme.textMuted}>Filter</text>
        <input
          placeholder="name, repo, state, path, summary…"
          placeholderColor={theme.textMuted}
          focusedBackgroundColor={theme.backgroundPanel}
          cursorColor={theme.primary}
          focusedTextColor={theme.text}
          onInput={setFilter}
        />
      </box>
      <Switch>
        <Match when={report.loading}>
          <text fg={theme.textMuted}>Loading Djinn sessions…</text>
        </Match>
        <Match when={report.error}>
          {(error) => <text fg={theme.error}>{String(error())}</text>}
        </Match>
        <Match when={true}>
          <box flexGrow={1} minHeight={0} flexDirection="row" gap={1}>
            <box flexGrow={1} minHeight={0} gap={1}>
              <For each={filteredSessions()} fallback={<text fg={theme.textMuted}>No Djinn sessions match.</text>}>
                {(session, index) => {
                  const active = () => session.path === selectedSession()?.path
                  const checkedLabel = () => (checked().has(session.path) ? "[x]" : "[ ]")
                  return (
                    <box
                      flexDirection="row"
                      gap={1}
                      paddingLeft={1}
                      paddingRight={1}
                      backgroundColor={active() ? theme.backgroundElement : theme.background}
                      onMouseDown={() => setSelectedPath(session.path)}
                    >
                      <text fg={checked().has(session.path) ? theme.primary : theme.textMuted} onMouseDown={() => toggleSession(session.path)}>
                        {checkedLabel()}
                      </text>
                      <text fg={theme.textMuted} flexShrink={0}>
                        {String(index() + 1).padStart(2, " ")}
                      </text>
                      <box flexGrow={1} minWidth={0}>
                        <text fg={active() ? theme.text : theme.text} wrapMode="none">
                          {sessionStateBadge(session)} {truncateMiddle(session.display_name || session.name, 66)}
                        </text>
                        <text fg={theme.textMuted} wrapMode="none">
                          {sessionListMetadata(session)}
                        </text>
                        <Show when={session.next_action}>
                          {(action) => <text fg={theme.warning}>Action: {truncateMiddle(action(), 80)}</text>}
                        </Show>
                      </box>
                    </box>
                  )
                }}
              </For>
            </box>
            <box flexGrow={1} minHeight={0} gap={1}>
              <Show when={selectedSession()} fallback={<text fg={theme.textMuted}>Select a session for preview and actions.</text>}>
                {(session) => (
                  <>
                    <box gap={1}>
                      <text fg={theme.text}>{session().display_name || session().name}</text>
                      <text fg={theme.textMuted}>Reference: {session().reference_name}</text>
                      <text fg={theme.textMuted}>Path: {session().path}</text>
                      <text fg={theme.textMuted}>Status: {sessionStateBadge(session())}</text>
                      <text fg={theme.textMuted}>Repo: {sessionRepoLabel(session())}</text>
                      <text fg={theme.textMuted}>Turns: {session().turn_count} · Events: {eventHealthLabel(session().event_health)}</text>
                      <Show when={session().candidates}>
                        {(candidates) => <text fg={theme.textMuted}>Candidates: {candidateStatus(candidates())}</text>}
                      </Show>
                    </box>
                    <box flexDirection="row" gap={1} flexShrink={0}>
                      <Show when={session().buddy?.buddy_session}>
                        {(id) => <text fg={theme.primary} onMouseDown={() => props.onOpenBuddySession(id())}>chat</text>}
                      </Show>
                      <text fg={theme.primary} onMouseDown={() => void runAndRefresh("Run session", ["session", "run", session().path])}>run</text>
                      <text fg={theme.primary} onMouseDown={() => void runAction("Watch session", ["session", "watch", session().path])}>watch</text>
                      <text fg={theme.primary} onMouseDown={() => void runAction("Open summary", ["session", "open", session().path, "summary"])}>summary</text>
                      <text fg={theme.primary} onMouseDown={() => void runAction("Edit request", ["session", "open", session().path, "request"])}>request</text>
                      <text fg={theme.primary} onMouseDown={() => void runAction("Open context", ["session", "open", session().path, "context"])}>context</text>
                      <text fg={theme.primary} onMouseDown={() => void runAndRefresh("Discover context", ["session", "context", "discover", session().path])}>discover</text>
                    </box>
                    <box flexDirection="row" gap={1} flexShrink={0}>
                      <text fg={theme.primary} onMouseDown={() => promote("memory")}>promote memory</text>
                      <text fg={theme.primary} onMouseDown={() => promote("todo")}>todo</text>
                      <text fg={theme.primary} onMouseDown={() => promote("skill")}>skill</text>
                      <text fg={theme.primary} onMouseDown={() => promote("pattern")}>pattern</text>
                    </box>
                    <box flexDirection="row" gap={1} flexShrink={0}>
                      <text fg={theme.primary} onMouseDown={() => void runAction("Validate events", ["session", "validate-events", session().path])}>validate events</text>
                      <text fg={theme.primary} onMouseDown={() => void runAction("Preview events", ["session", "events", session().path])}>events</text>
                      <text fg={theme.primary} onMouseDown={() => void runAndRefresh("Rebuild events", ["session", "events", session().path, "--write"])}>rebuild</text>
                    </box>
                    <Show when={candidate()}>
                      {(item) => (
                        <box gap={1} flexShrink={0}>
                          <text fg={theme.text}>Candidate: {item().id} · {item().status}</text>
                          <box flexDirection="row" gap={1}>
                            <text fg={theme.primary} onMouseDown={() => void open(item().path).catch((error) => toast.error(error))}>open</text>
                            <text fg={theme.primary} onMouseDown={() => void runAction("Validate candidate", ["session", "validate-candidates", session().path, item().id])}>validate</text>
                            <text fg={theme.primary} onMouseDown={() => void runAndRefresh("Accept candidate", ["session", "accept", session().path, item().id])}>accept</text>
                            <text fg={theme.primary} onMouseDown={() => void runAndRefresh("Accept candidate and sync MindWeaver", ["session", "accept", session().path, item().id, "--sync-mindweaver"])}>accept + mw</text>
                            <text fg={theme.primary} onMouseDown={() => void runAndRefresh("Deny candidate", ["session", "deny", session().path, item().id])}>deny</text>
                          </box>
                        </box>
                      )}
                    </Show>
                    <Show when={session().summary_preview}>
                      {(summary) => <text fg={theme.text}>{truncateMiddle(summary(), 1000)}</text>}
                    </Show>
                  </>
                )}
              </Show>
            </box>
          </box>
        </Match>
      </Switch>
      <text fg={theme.textMuted} flexShrink={0}>
        {truncateMiddle(status(), 180)}
      </text>
    </box>
  )
}

function DashboardPlaceholderTab(props: { tab: Exclude<AppTabID, "chat" | "sessions"> }) {
  const { theme } = useTheme()
  return (
    <box flexGrow={1} minHeight={0} paddingLeft={2} paddingRight={2} paddingTop={1} gap={1}>
      <text fg={theme.text}>{titlecase(props.tab)}</text>
      <text fg={theme.textMuted}>{djinnDashboardTabNotes[props.tab]}</text>
      <text fg={theme.textMuted}>This tab is intentionally present now so Tab/Shift+Tab reserve Buddy-wide dashboard navigation.</text>
    </box>
  )
}

type DjinnSessionReport = {
  root?: string
  sessions?: DjinnFolderSession[]
}

type DjinnFolderSession = {
  name: string
  display_name: string
  reference_name: string
  path: string
  lifecycle: {
    state: string
    mode?: string
  }
  updated_at?: string
  modified_at?: string
  workspace?: string
  repo_path?: string
  summary_preview?: string
  turn_count: number
  event_health: {
    ready: boolean
    events_exists: boolean
    event_count: number
    event_turn_count: number
    issue_count: number
    issue_codes: string[]
  }
  buddy?: {
    buddy_session?: string
    command?: string
    last_run_at?: string
    runtime_path: string
  }
  candidates?: {
    total: number
    accepted: number
    denied: number
    pending: number
    entries: DjinnCandidate[]
  }
  next_action?: string
}

type DjinnCandidate = {
  id: string
  candidate_type?: string
  status: string
  path: string
  text?: string
  rationale?: string
  evidence: string[]
  destination?: string
  writeback_path?: string
}

async function loadDjinnSessionReport() {
  const raw = await runDjinn(["session", "ls", "--json"])
  return JSON.parse(raw) as DjinnSessionReport
}

async function runDjinn(args: string[]) {
  const proc = Bun.spawn([process.env.DJINN_BIN ?? "djinn", ...args], {
    stdout: "pipe",
    stderr: "pipe",
  })
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ])
  if (exitCode === 0) return stdout
  throw new Error(stderr.trim() || stdout.trim() || `djinn ${args.join(" ")} exited with ${exitCode}`)
}

function compareDjinnSessions(left: DjinnFolderSession, right: DjinnFolderSession) {
  return sessionRepoLabel(left).localeCompare(sessionRepoLabel(right)) || (right.updated_at ?? "").localeCompare(left.updated_at ?? "") || left.display_name.localeCompare(right.display_name)
}

function sessionListMetadata(session: DjinnFolderSession) {
  const mode = session.lifecycle.mode ?? "-"
  const updated = session.updated_at ?? session.modified_at ?? "unknown"
  const candidates = session.candidates ? ` · candidates ${candidateStatus(session.candidates)}` : ""
  return `${mode} · ${session.turn_count} turns · events ${eventHealthLabel(session.event_health)}${candidates} · updated ${updated}`
}

function sessionRepoLabel(session: DjinnFolderSession) {
  if (!session.repo_path?.trim()) return "No linked repo"
  return session.repo_path.split(/[\\/]/).filter(Boolean).at(-1) ?? session.repo_path
}

function sessionStateBadge(session: DjinnFolderSession) {
  const state = session.lifecycle.state
  const mode = session.lifecycle.mode
  if (state === "running" && mode === "promotion") return "▶ generating"
  if (state === "running") return "▶ running"
  if (state === "failed" && mode === "promotion") return "⚠ promotion failed"
  if (state === "failed") return "⚠ failed"
  if (state === "completed" && mode === "promotion") return "✓ candidates ready"
  if (state === "completed") return "✓ completed"
  if (state === "paused") return "Ⅱ paused"
  if (state === "not_started" && mode === "promotion") return "○ promotion draft"
  if (state === "not_started") return "○ draft"
  if (mode) return `${state}/${mode}`
  return state
}

function eventHealthLabel(health: DjinnFolderSession["event_health"]) {
  if (!health.events_exists) return "missing"
  if (health.ready) return `${health.event_count} ready`
  if (health.issue_codes.length === 0) return `${health.issue_count} issue${health.issue_count === 1 ? "" : "s"}`
  return health.issue_codes.join(", ")
}

function candidateStatus(candidates: NonNullable<DjinnFolderSession["candidates"]>) {
  return `${candidates.total} total, ${candidates.accepted} accepted, ${candidates.denied} denied, ${candidates.pending} pending`
}

function truncateMiddle(value: string, max: number) {
  if (value.length <= max) return value
  const half = Math.floor((max - 1) / 2)
  return `${value.slice(0, half)}…${value.slice(value.length - half)}`
}

function titlecase(value: string) {
  return value.slice(0, 1).toUpperCase() + value.slice(1)
}
