# MCP Server

A Xuan plugin that lets LLM clients such as Claude Code, Claude Desktop and
Codex see and edit your open documents through the [Model Context
Protocol](https://modelcontextprotocol.io). It serves MCP Streamable HTTP on
`127.0.0.1` only, behind a bearer token. What the tools do, how to connect a
client and the security model are in
[docs/AGENTS-GUIDE.md](../../docs/AGENTS-GUIDE.md); the design is in
[docs/MCP.md](../../docs/MCP.md).

## Install

**Xuan's release packages include this plugin**, in the bundled plugins folder
next to Xuan (see "Bundled plugins" in
[docs/PLUGINS.md](../../docs/PLUGINS.md#bundled-plugins)):

| Package | Folder |
| --- | --- |
| deb, rpm | `/usr/lib/xuan/plugins/mcp-server/` |
| Linux tar.gz | `lib/xuan/plugins/mcp-server/` (`scripts/install.sh` copies it to `~/.local/lib/xuan/plugins/`) |
| AppImage | `usr/lib/xuan/plugins/mcp-server/` inside the image |
| Windows zip | `plugins\mcp-server\` next to `xuan.exe` |

It is off until you allow it. Xuan asks before the plugin first runs, showing
that it edits documents, asks before each client session's first edit, and is
treated as a network plugin (it listens on `127.0.0.1`). The grant carries over
when you upgrade Xuan, unless the plugin's command or permissions change.

## Build from source

To run a build of your own (Rust 1.88 or later):

```sh
cd plugins/mcp-server
cargo build --release
```

Then either point Xuan at the repository's plugins with
`XUAN_PLUGIN_PATH=/path/to/xuan/plugins`, or install this folder with
**Plugins → Install from Folder or Zip…** after building (the build output in
`target/` is large; copying just `plugin.toml` and
`target/release/xuan-mcp-server` (`.exe` on Windows) into a folder of their own
is enough). Rebuild after updating the source.

Your copy replaces the bundled one: with the same id `mcp-server`, Xuan loads
yours and skips the bundled copy, without reporting a conflict. As it is in
another folder, Xuan asks for permission again. Remove your copy to go back to
the bundled plugin.

The release packages build it with `cargo build --release --locked` (on Linux
for the `x86_64-unknown-linux-musl` target, so the binary is static and also
runs from the AppImage on older systems; `rustup target add
x86_64-unknown-linux-musl` before `scripts/package.sh`).

## Use

Open **Window → MCP Server**. The pane shows the address and has **Copy**
buttons for the token, the `claude mcp add` command and a JSON `mcpServers`
entry. The port is a setting in **Plugins → Manage Plugins… → MCP Server**
(default 8765). If another program already listens there, the pane warns you
and waits: clients set up for that port may be talking to the other program.

## Files

The plugin keeps in its data folder (`<config dir>/plugin-data/mcp-server/`):

- `token`: the bearer token, readable only by you. **New Token** in the pane
  replaces it.
- `connection.json`: the address it listens on.
- `incoming/`: images a client sends for `create_image_layer`, removed once
  Xuan has read them.

## Tests

```sh
cargo test           # HTTP checks, MCP conformance and tools against a fake editor
cargo clippy --all-targets -- -D warnings
```

CI runs both, with `cargo fmt --check`, on Linux and Windows (the
`mcp-server` job in `.github/workflows`).

Xuan's own test suite runs this plugin against a real headless editor when the
release binary exists (or `XUAN_MCP_SERVER` names it):
`the_mcp_server_plugin_drives_the_editor_over_http`.

## Dependencies and licences

This plugin's dependencies stay out of Xuan's own build. The main ones are the
official Rust MCP SDK [`rmcp`](https://crates.io/crates/rmcp) (Apache-2.0),
[`axum`](https://crates.io/crates/axum) and [`tokio`](https://crates.io/crates/tokio)
(MIT), [`base64`](https://crates.io/crates/base64) and
[`getrandom`](https://crates.io/crates/getrandom) (MIT or Apache-2.0); the full
list is in `Cargo.lock`. The plugin itself is MIT, like Xuan.
