# Running other plugins' actions from MCP (issue #124)

An MCP client can paint, mask and filter through the MCP server plugin, but
cannot run another installed plugin's actions (Comfy Cloud's Generate Image…,
Edit Image…, Split into Layers, the AI Region actions). `host/run` refuses
another plugin's action on purpose, and `run_command` takes only built-in
commands. This plan keeps that refusal as the default and adds an explicit,
visible exception the user turns on.

## Decisions

1. **Two grant settings, both off by default and dropped with the grant.**
   `run_other_actions` ("Run other plugins' actions") lets a plugin list and
   start other plugins' actions at all; without it `plugins/actions`,
   `jobs/list` and a `host/run` of another plugin's action fail with an error
   that names the setting. `run_without_asking` is what **Always Allow** in
   the run prompt stores; it skips the per-session prompt. The issue names one
   setting and an Always Allow that "sets the grant"; both cannot be one flag
   (with the setting off the request fails, so the prompt could never offer
   to turn it on), so they are the same pair as edits have
   (`edit_prompt = "session"` plus `edit_without_asking`). Both live in
   `PluginGrant` next to `save_without_asking`, are copied only while the
   grant covers the same folder, command and permissions, and show in
   **Plugins → Manage Plugins…** for plugins that ask before edits (the MCP
   server) and for any plugin while they are on. The setting is generic: any
   plugin may be given it, though only the MCP server is offered it.
2. **A prompt per session and target plugin.** The first run in a session
   holds the request and asks **Run “Generate Image” for MCP Server (plugin
   mcp-server)?**, naming the target plugin with its id, the session, the
   text inputs and, for a network plugin, its hosts and that a run may send
   document data and use paid credits. **Allow** covers that session's runs
   of that target plugin until the caller stops (keyed by caller, session and
   target, so allowing Comfy Cloud does not allow every plugin); **Always
   Allow** sets `run_without_asking`; **Cancel** refuses this run with
   `-32800` and starts the 30-second cooldown edits and files use, during
   which new runs are refused without a prompt. It is modelled on the edit
   and file prompts: it waits until no other dialog, job or action is open,
   comes back after another dialog replaced it (#56), `request/cancel`
   closes it and a late answer does nothing (#57). One run per caller may
   wait for the prompt; a second fails at once, so runs that cost money
   cannot stack up behind one answer.
3. **The target's own checks, in the menu's order, after the prompt.** The
   run uses the target's grant, enabled state, offline mode, models and send
   prompt, never the caller's. Disabled, offline or unknown targets and
   actions fail before the prompt, so the user is not asked about a run that
   cannot happen. After the prompt: busy (a dialog, a job, an open action or
   Develop), offline (with the menu's status message), not granted (the
   permission prompt the menu shows, with nothing to resume, as surfaces do;
   the caller is told to try again once allowed), the layers to select, the
   start problems the menu reports ("Select an image layer first"), models
   (the download prompt; after the download the action's dialog opens with
   the caller's inputs, as for a menu run that waited), then
   `run_plugin_action`, whose send prompt (#39) lists exactly what goes to a
   network plugin. Nothing runs without a prompt that a menu run would show.
4. **Not through `start_plugin_action_with`.** The issue says the run starts
   through `start_plugin_action_with`, but that opens the action's dialog for
   any action with inputs, reports problems only to the user, and resumes a
   granted plugin as a menu run. A run for another plugin takes the path
   surface runs take (`run_from_surface`): the same checks, an `ActionEdit`
   marked with its caller (no dialog), and `run_plugin_action`.
5. **The request is answered when the job starts.** `host/run` answers
   `{ok, running: true, job, plugin, plugin_name, action, label}` once the
   job runs, or an error that says why nothing ran. While the target's send
   prompt is up, the answer waits (the MCP server keeps the client informed
   with progress, as for every prompt); **Send** answers with the job,
   **Cancel**, closing the action or the caller stopping answers `-32800`, and
   `request/cancel` closes the send prompt. Failures go to the caller only,
   never as an error dialog for the user.
6. **Inputs are checked strictly.** `Action::check_inputs` refuses unknown
   inputs, wrong types, values not among an enum's, numbers outside
   `min`/`max` or not whole for integers, colours that are not `#rrggbb(aa)`,
   texts over 64 KiB and malformed regions or region counts outside
   `min`/`max`, naming the input. Missing inputs take their defaults. A
   caller may not set `path` inputs (only the user chooses files for another
   plugin; a client could otherwise have a network plugin read and upload
   any file) or `secret` inputs.
7. **`into` for `result = { into = "ask" }`.** Required for such actions, and
   only `layer` or `document`, the choices the action's dialog offers (the
   issue also lists `replace`, which the dialog never offers for `ask`
   actions). `into` with any other action is refused. `layer` needs an open
   document.
8. **`plugins/actions`** lists the actions of installed, enabled plugins
   other than the caller: plugin id and name, whether it uses the network,
   is allowed to run and is available (not offline), and per action its id,
   `<plugin>/<id>`, label, description, kind, source, surfaces, result and
   declared inputs as the manifest declares them. It needs only the setting.
9. **`jobs/list`** reports what the status bar shows: every running action,
   import, export and model download in status bar order, with label,
   progress, message, document, which one the bar shows ("1 of N") and the
   plugin that started it; how many finished results wait for the editor;
   the open proposal; and the caller's own recent runs that ended:
   `waiting`, `proposed`, `done`, `accepted`, `discarded`, `failed` (with the
   error) or `cancelled`. Other plugins' outcomes are not reported. It needs
   the setting, as it exists to follow these runs.
10. **Visibility.** The job runs in the status bar like any job, with Cancel;
    hovering it says which plugin started it. The caller's log notes each run
    it started. The MCP Server pane lists "run_plugin_action: started
    Generate Image… from Comfy Cloud" (names from Xuan's answer, cleaned like
    file names), and says when clients may run other plugins' actions.
11. **MCP tools.** `list_plugin_actions` wraps `plugins/actions` and turns
    each action's inputs into a JSON schema (`path` inputs left out).
    `run_plugin_action` (`plugin`, `action`, `inputs`, `into`, `layers`)
    sends `host/run` and returns at once with the job; it is marked
    open-world, as the target may use the network. `get_jobs` wraps
    `jobs/list`. All three are `Run` tools, so `batch` refuses them.

## Data flow

```
run_plugin_action ─▶ host/run {action: "comfy-cloud/generate", inputs, into, layers, session}
  dispatch_plugin_message ─▶ hold_plugin_run
    plugin_run(): setting? target, action, enabled, offline, inputs, into, layers
    allowed for (caller, session, target), or run_without_asking? ─ no ─▶ held; Run prompt
       Allow / Always Allow ─▶ release_held_runs ─▶ start_plugin_run    Cancel ─▶ -32800
    start_plugin_run(): busy, offline, granted (else permission prompt), select layers,
       start problem, models (else download prompt) ─▶ ActionEdit{caller} ─▶ run_plugin_action
         send prompt (network) ─▶ answered on Send / Cancel / withdraw
         job started ─▶ {ok, running, job, …}
  PluginJob{caller} ─▶ status bar; finished ─▶ completed ─▶ proposal ─▶ Accept / Discard
  jobs/list ◀─ get_jobs: running, waiting, proposal, the caller's outcomes
```

## Tests

Xuan (`src/app/tests/plugins.rs`, `src/plugins/manifest.rs`):

- input checks: types, enums, ranges, whole numbers, colours, texts,
  regions, unknown, `path` and `secret` inputs (unit tests);
- refused without the setting, with the setting named; `plugins/actions`
  lists other enabled plugins' actions with their inputs;
- unknown plugin or action, disabled or offline target refused before any
  prompt; `into` required for `ask`, `replace` and `into` for other actions
  refused;
- the prompt: held, Allow starts the job and answers with it, the session
  is remembered per target, Always Allow sets the grant, Cancel answers
  `-32800` and cools down; withdrawn closes it and a late answer does
  nothing; it comes back after another dialog (UI test);
- the target's checks: not granted shows its permission prompt and runs
  nothing; a network target asks before sending, Send answers with the job,
  Cancel answers `-32800`;
- the grant settings are dropped when the plugin changes, and switching them
  off in Manage Plugins forgets the sessions;
- `jobs/list`: running jobs with their caller, outcomes from waiting to
  proposed to accepted.

MCP server (`plugins/mcp-server/src/tests.rs`): the input schemas, the
`host/run` params, argument checks, refusals explained, the pane's line,
`get_jobs`, the three tools refused in `batch`, annotations and wait
messages.

## Commits

1. `docs(plans)`: this plan.
2. `feat(plugins)`: strict input checks in the manifest.
3. `feat(plugins)`: the grant settings, `plugins/actions`, runs of other
   plugins' actions with the prompt and the target's checks.
4. `feat(plugins)`: `jobs/list`, run outcomes and the status bar's
   attribution.
5. `feat(mcp)`: the three tools and the pane.
6. `docs`: PLUGINS.md, MCP.md and AGENTS-GUIDE.md.

## Out of scope

Plugins other than the MCP server being offered the setting, and Xuan as an
MCP client of outside servers.
