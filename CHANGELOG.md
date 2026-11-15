# Changelog

Versions follow [Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.
The version is set in one place, `[workspace.package] version` in the root `Cargo.toml`;
it ends up in the window ("v0.1.0" next to the title), the tray tooltip, the file
properties of `rerout.exe` and the build output. The config file has its own
`version` field (see `CONFIG_VERSION` in `crates/common/src/config.rs`).

## [0.2.0]
- Connections window: **Close** a TCP connection (or all connections on not-allowed interfaces) so the app
  reconnects through the current rule. Windows only allows this for IPv4 TCP and with administrator
  rights; the window offers "Restart as administrator".
- Clicking Connections again brings an already open Connections window to the front.

## [0.1.0]
First versioned build.
- Per-application interface rules: Allowed / Denied columns with drag & drop, priority and failover,
  loopback allow/block, rules apply to running apps without a restart.
- Add apps from the running processes list (icon, sockets, interfaces in use), by browsing or by
  drag & drop of an .exe / shortcut.
- Connections window for an app, Launch with the rule applied from the first packet.
- Tray icon, start with Windows, single instance, hide to tray on close.
- Application icon (window, tray and the exe file), version information.

## How to release
1. Bump `version` in the root `Cargo.toml`.
2. Add a section here.
3. Run `build.cmd` (it prints the version) and ship the `dist` folder.
