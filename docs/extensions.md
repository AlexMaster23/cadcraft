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
the manifest command's `function`; input and output are bounded JSON strings. The initial
[WIT contract](../crates/engine/wit/cadcraft-extension.wit) exposes only guarded transactions and
line creation. New drawing capabilities will be added deliberately and versioned.

The native runtime limits every invocation to 10 million WebAssembly fuel units, 64 MiB memory,
10,000 drawing operations, and 1 MiB input/output JSON. A command may open one transaction. Host
operations remain staged until the guest returns successfully after commit; a trap, fuel
exhaustion, error, missing commit or invalid output leaves the drawing unchanged. A successful
transaction becomes one normal CADCraft undo step.

Command IDs and aliases cannot shadow CADCraft commands, built-in aliases or another enabled
extension. Disabling or uninstalling an extension removes its declared commands from every command
surface immediately.

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
