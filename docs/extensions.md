# CADCraft extensions

CADCraft desktop can install portable `.cadcraft-extension` bundles. An extension contributes
commands and menu items through a validated manifest and implements them in CADCraft's AutoLISP
runtime. The same command is then available from the command line, menus, scripts, the control
channel, MCP and `cadcraft-cli`.

This is schema 1. It deliberately avoids native dynamic libraries: Rust has no stable plugin ABI,
native libraries would not work in the browser, and loading them would give extension code the
same process privileges as CADCraft. The package format leaves room for another portable runtime,
such as WebAssembly, in a later schema.

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

The desktop and headless CLI load installed extensions. The web build understands the portable
runtime and package types, but schema 1 has no browser-side persistent installer.

## Build a package

Copy [the hello example](../examples/extensions/hello), edit it, and pack the directory:

```sh
cadcraft-cli extensions pack examples/extensions/hello hello.cadcraft-extension
```

The packer and installer both validate the manifest, portable paths, duplicate names, entry point
and size limits. Packages may contain at most 128 regular files, 8 MiB uncompressed in total and
2 MiB per ordinary file. Symlinks and path traversal are rejected.

## Manifest

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

Command IDs and aliases cannot shadow CADCraft commands, built-in aliases or another enabled
extension. Disabling or uninstalling an extension removes its declared commands from every command
surface immediately.

## Permissions and trust

Reading and editing the active drawing is intrinsic to an extension. Direct AutoLISP file access
through `open`, `findfile`, filesystem-backed `load` and APPLOAD requires `read-files` or
`write-files` as appropriate. Schema 1 exposes no process-launch or network API.

Extensions are source-visible but are not cryptographically signed, and schema 1 is not an
adversarial security sandbox: an extension can invoke CAD commands available to a user and can
change the active drawing. Install extensions only from sources you trust and review the manifest
and `main.lsp` when security matters.

## Runtime lifecycle

CADCraft validates the complete replacement catalog before activating it. A rejected update leaves
the previous working catalog active. Entry-point parse/runtime failures are reported in the command
history and are not retried every frame; Reload, enable, or reinstall retries them. Each drawing has
its own AutoLISP environment, matching normal CAD LISP behavior.
