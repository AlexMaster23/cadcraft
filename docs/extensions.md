# CADCraft extensions

CADCraft desktop can install portable `.cadcraft-extension` bundles. An extension contributes
commands and menu items through a validated manifest and implements them in CADCraft's AutoLISP
runtime (schema 1) or capability-based WebAssembly component runtime (schema 2). The same command
is then available from the command line, menus, scripts, the control channel, MCP and
`cadcraft-cli`.

Neither schema loads native dynamic libraries: Rust has no stable plugin ABI, native libraries
would not work in the browser, and loading them would give extension code the same process
privileges as CADCraft. Schema 2 uses the WebAssembly Component Model and a small versioned WIT
contract instead.

## Install and manage

In the desktop app, open **Tools > Extensions...**, choose **Install...**, and select a
`.cadcraft-extension` file. The same window enables, disables, reloads and uninstalls extensions.
Restarting is not required. Entry points load once in every drawing while their extension is
enabled.

The CLI exposes the same per-user store:

```sh
cadcraft-cli extensions list
cadcraft-cli extensions install hello.cadcraft-extension
cadcraft-cli extensions disable org.cadcraft.hello
cadcraft-cli extensions enable org.cadcraft.hello
cadcraft-cli extensions uninstall org.cadcraft.hello
```

Installed files live below the CADCraft settings directory in `extensions/<extension-id>/`:
`%APPDATA%\CADCraft` on Windows, `~/Library/Application Support/CADCraft` on macOS, and
`$XDG_CONFIG_HOME/cadcraft` or `~/.config/cadcraft` on Unix. `CADCRAFT_CONFIG_DIR` overrides the
settings directory for both the app and CLI.

The desktop and headless CLI load installed extensions. The web build understands manifests and
packages, but it does not yet provide the schema 2 component runtime or a persistent installer.

## Build a package

Copy [the hello example](../examples/extensions/hello), edit it, and pack the directory:

```sh
cadcraft-cli extensions pack examples/extensions/hello hello.cadcraft-extension
```

The packer and installer both validate the manifest, portable paths, duplicate names, entry point
and size limits. Packages may contain at most 128 regular files, 64 MiB uncompressed in total and
32 MiB per ordinary file. Symlinks and path traversal are rejected.

## Schema 1 manifest

Every package has `extension.json` at its root:

```json
{
  "schemaVersion": 1,
  "id": "org.example.my-extension",
  "name": "My Extension",
  "version": "1.0.0",
  "cadcraft": ">=0.7.0, <1.0.0",
  "description": "What this extension does.",
  "license": "MIT",
  "entrypoint": "main.lsp",
  "permissions": ["read-files", "write-files"],
  "homepage": "https://example.org/my-extension",
  "commands": [
    {
      "id": "mycommand",
      "label": "My Command",
      "menu": ["Extensions", "My Command"],
      "aliases": ["MC"],
      "params": "{value: number} -> number",
      "function": "MYEXT:RUN",
      "interactive": true
    }
  ]
}
```

- `id` is a lower-case reverse-DNS identifier and identifies the installation across updates.
- `cadcraft` is a semantic-version requirement. Incompatible packages are rejected on install and
  incompatible installed packages remain disabled.
- `function` handles JSON/programmatic calls. It receives one AutoLISP value. JSON objects become
  association lists with string keys, arrays become lists, and scalars become their matching LISP
  values. Its return value is converted to JSON.
- An interactive command also defines `C:<id>` in its entry point. It may use normal prompt
  functions and CADCraft commands.
- `menu` is the complete menu path. Its first segment may add an extension-owned top-level menu.

## Schema 2 WebAssembly manifest

Schema 2 replaces the top-level AutoLISP `entrypoint` with a component runtime and requires
explicit drawing capabilities:

```json
{
  "schemaVersion": 2,
  "id": "org.example.survey-tools",
  "name": "Survey tools",
  "version": "1.0.0",
  "cadcraft": ">=0.7.0, <1.0.0",
  "description": "Portable surveying commands.",
  "license": "MIT OR Apache-2.0",
  "runtime": {
    "kind": "wasm-component",
    "entrypoint": "survey_tools.wasm",
    "world": "cadcraft:extension/cadcraft-extension@0.1.0"
  },
  "permissions": ["drawing-read", "drawing-write"],
  "commands": [
    {
      "id": "survey.reference-line",
      "label": "Create reference line",
      "menu": ["Survey", "Create reference line"],
      "function": "reference-line",
      "params": "{} -> {entityId}"
    }
  ]
}
```

CADCraft calls the component world's `run-command(command, parameters-json)` export. `command` is
the manifest command's `function`; input and output are bounded JSON strings. The
[WIT contract](../crates/engine/wit/cadcraft-extension.wit) exposes guarded transactions, native
line/arc/circle/point/text and polygonal solid-fill creation, bounded entity color and lineweight
appearance, namespaced metadata, entity queries, and guarded point/text updates and deletion. Solid
fills are stored as ordinary native `HATCH` entities. New drawing capabilities are added deliberately
and versioned.

The native runtime limits every invocation to 10 million WebAssembly fuel units, 64 MiB memory,
10,000 drawing operations, and 1 MiB input/output JSON. A command may open one transaction. Host
operations remain staged until the guest returns successfully after commit; a trap, fuel
exhaustion, error, missing commit or invalid output leaves the drawing unchanged. A successful
transaction becomes one normal CADCraft undo step.

### Persistent extension data

Schema 2 extensions with `drawing-read` can read their own drawing/entity metadata and query
entities by one metadata key/value. With `drawing-write`, the same transaction used for geometry
can set or remove metadata, update point/text entities and delete entities. CADCraft scopes every
metadata call to the current extension ID: a guest never supplies a namespace and cannot read or
overwrite another extension's values.

Drawing metadata is stored in a bounded `CADCRAFT_EXTENSIONS` XRECORD. Entity metadata is stored in
bounded `CADCRAFT_EXTENSIONS` xdata and follows the entity through normal save/reopen and copy
behavior. Metadata mutations remain staged with geometry until commit, and roll back together on
failure. Entity IDs returned by create calls are reserved drawing handles, so a guest can persist
exact links in drawing metadata during the same transaction.

Command IDs and aliases cannot shadow CADCraft commands, built-in aliases or another enabled
extension. Disabling or uninstalling an extension removes its declared commands from every command
surface immediately.

## Host-rendered command tables

A non-interactive command can declare an optional native table dialog. CADCraft owns and renders
the window; the extension supplies only bounded labels, columns and validation metadata:

```json
{
  "id": "survey.points.import",
  "label": "Import points...",
  "menu": ["Survey", "Points", "Import points..."],
  "aliases": ["SURVEYPOINTS"],
  "function": "insert-points",
  "interactive": false,
  "ui": {
    "kind": "table",
    "title": "Import points",
    "description": "Enter rows or open a coordinate file, then review them before insertion.",
    "submitLabel": "Insert points",
    "accent": "#F47C20",
    "selects": [
      {
        "key": "convention",
        "label": "Coordinate convention",
        "default": "cad-xy",
        "choices": [
          { "value": "cad-xy", "label": "CAD / GIS X-Y" },
          { "value": "survey-xy", "label": "Survey X-Y" }
        ]
      }
    ],
    "columns": [
      {
        "key": "number",
        "label": "Point number",
        "kind": "text",
        "required": true,
        "unique": true,
        "aliases": ["point", "id"]
      },
      {
        "key": "easting",
        "label": "Easting (X)",
        "kind": "number",
        "required": true,
        "aliases": ["e", "x"]
      }
    ]
  }
}
```

`kind` is `text` or `number`; `required` and `unique` are optional booleans. Column keys must be
unique and become keys in the submitted JSON row. `aliases` are case-insensitive CSV header names.
The optional accent is a `#RRGGBB` color. Manifests may declare 1–12 columns.

`selects` is optional and may contain up to eight host-rendered drop-downs. Every selector has
2–16 bounded choices and a default that matches one choice value. Selector keys are unique and
cannot be `rows`; the selected values become top-level fields in the submitted JSON. This lets an
extension ask for conventions, units or import modes without running extension-owned UI code.

A menu, toolbar or typed-command invocation opens the table. Rows can be entered manually or
loaded from a user-selected UTF-8 CSV/text file; comma, semicolon and tab delimiters are supported.
CADCraft limits input to 2,000 rows, 64 KiB per field and 8 MiB per selected file. Only validated
non-empty rows are submitted as `{"convention":"cad-xy","rows":[...]}` when the example selector
is present, or `{"rows":[...]}` when there are no selectors. Programmatic invocation through `CadApp::run`,
the control channel or CLI remains dialog-free and passes the caller's JSON directly to the guest.

File selection is a host action: a schema 2 guest receives only the submitted JSON and gains no
ambient file-system access. Declarative tables are intentionally narrower than arbitrary
extension-owned windows.

## Host-rendered data managers

A command can declare a bounded native manager for existing extension-owned records. As with
tables, CADCraft owns the renderer, validation, drawing selection/zoom and save-file dialog; the
extension receives explicit JSON actions through its ordinary command function:

```json
{
  "id": "survey.points.manager",
  "label": "Point Manager...",
  "menu": ["Survey", "Points", "Point Manager..."],
  "aliases": ["POINTMANAGER"],
  "function": "manage-points",
  "interactive": false,
  "ui": {
    "kind": "manager",
    "title": "Survey — Point Manager",
    "description": "Review and edit linked survey points.",
    "accent": "#F47A20",
    "idKey": "id",
    "entityIdsKey": "entities",
    "renumberKey": "number",
    "allowDelete": true,
    "allowExport": true,
    "allowAdd": false,
    "allowEntityActions": true,
    "allowSynchronize": true,
    "allowPreview": false,
    "columns": [
      {
        "key": "number",
        "label": "Point number",
        "kind": "text",
        "editable": true,
        "required": true,
        "unique": true,
        "width": 120
      },
      {
        "key": "status",
        "label": "Link status",
        "kind": "text",
        "width": 110
      }
    ]
  }
}
```

Managers have 1–16 unique columns. `kind` is `text` or `number`; `editable`, `required` and
`unique` default to false, and read-only columns cannot be required or unique. Width is 60–400
logical pixels. `idKey` identifies a bounded unique string on every row; `entityIdsKey` identifies
an array of drawing handles used by **Select in drawing** and **Locate**. The optional
`renumberKey` must name an editable text column.

`allowAdd` appends host-owned blank records and sends them through `update`; temporary record ids
begin with `__cadcraft_new_`. `allowEntityActions` and `allowSynchronize` independently hide the
drawing-selection/location and synchronize controls for non-geometric record sets. `allowPreview`
adds a non-mutating preview action. These four options default to false, true, true and false,
respectively, so existing managers retain their behavior.

Opening a manager calls the command with `{"action":"list"}`. Its response contains `rows` and
may contain a `summary`. CADCraft accepts at most 2,000 rows, 64 KiB per field and 32 drawing
handles per row, virtualizes the visible body and validates required/numeric/unique fields before
calling the extension. Host actions use these payloads:

| Action | Request fields | Purpose |
|---|---|---|
| `update` | `rows`: changed objects only | Commit staged cell edits |
| `preview` | `rows`: every current object | Return the same record ids with refreshed read-only columns and `summary`; editable cells remain dirty |
| `synchronize` | `ids` | Repair or refresh selected records and linked entities |
| `renumber` | `ids`, `prefix`, `start`, `step` | Deterministically renumber selected records |
| `delete` | `ids` | Delete selected records after host-side two-click confirmation |
| `export` | `ids` (empty means all) | Return `fileName` and UTF-8 `content` for host-owned saving |

After a successful mutation, the command returns a fresh `rows` snapshot. Export content is capped
at 8 MiB; filenames are portable basename components. The guest never learns the chosen save path
and still has no ambient file access. Programmatic command invocation remains dialog-free and can
send the same action payloads directly.

## Tool Sets contributions

An extension can place its declared commands in bounded, host-rendered groups on CADCraft's
Drafting or Modeling Tool Sets tab. The buttons start the exact same command used by menus, typed
aliases, scripts and automation:

```json
{
  "toolsets": [
    {
      "tab": "drafting",
      "group": "Survey tools",
      "position": "top",
      "accent": "#F47A20",
      "tools": [
        {
          "command": "survey.points.import",
          "icon": "import",
          "size": "large"
        }
      ]
    }
  ]
}
```

`tab` is `drafting` or `modeling`; `position` is `top` or `bottom` and defaults to `bottom`.
`size` is `large` or `small` and defaults to `large`. Every `command` must be declared by the same
extension and may be placed only once. A manifest may declare at most 16 groups and 32 tools per
group. The optional accent is a `#RRGGBB` color used for a small native group/button marker.

Icons are original CADCraft artwork selected by semantic name: `new`, `open`, `save`, `import`,
`export`, `attach`, `share`, `properties`, `layers`, `line`, `polyline`, `circle`, `arc`,
`rectangle`, `polygon`, `ellipse`, `spline`, `point`, `hatch`, `region`, `table`, `text`, `move`,
`copy`, `rotate` or `settings`. Extensions do not inject arbitrary drawing or UI code into the
palette. Users can collapse an extension group and use its gear menu to hide, restore or show its
buttons exactly like built-in Tool Sets groups.

## Permissions and trust

Schema 1 AutoLISP can read and edit the active drawing. Direct file access through `open`,
`findfile`, filesystem-backed `load` and APPLOAD requires `read-files` or `write-files` as
appropriate.

Schema 2 has no ambient drawing or file access. `drawing-read` and `drawing-write` authorize only
the matching WIT host calls. The initial contract exposes no file-system, process-launch or network
API.

Extensions are not yet cryptographically signed. Schema 1 is not an adversarial security sandbox:
AutoLISP can invoke CAD commands available to a user. Schema 2 is sandboxed by WebAssembly and its
declared capabilities, but an authorized drawing command can still change a drawing. Review the
publisher, manifest and requested permissions before installation.

## Runtime lifecycle

CADCraft validates the complete replacement catalog before activating it. A rejected update leaves
the previous working catalog active. Schema 1 entry-point failures are reported in the command
history and are not retried every frame; Reload, enable, or reinstall retries them. Each drawing has
its own AutoLISP environment. Schema 2 components are instantiated per invocation with fresh fuel,
memory and transaction state.
