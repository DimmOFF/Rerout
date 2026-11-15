# Rerout - per-application network interface control for Windows

![icon](assets/icon.png)

## Build (Windows, Rust + MSVC Build Tools)
```bat
build.cmd          :: x64 only
build.cmd all      :: x64 + x86 (also for 32-bit applications)
```
Output goes to `dist\`: `rerout.exe`, `rerout_hook_x64.dll`, `rerout-injector-x64.exe`
(plus the `x86` variants with `all`). Keep these files in one folder.
Settings: `%APPDATA%\rerout\config.json`.
The icon (`crates/app/rerout.ico`) and version info (`rerout.rc`) are embedded into `rerout.exe`
by `build.rs` (needs `rc.exe` from the Windows SDK, part of the MSVC Build Tools).

## Versioning
The version lives in one place: `[workspace.package] version` in the root `Cargo.toml`. It is shown in the
window (hover for build date and commit), the tray tooltip and the properties of `rerout.exe`.
See `CHANGELOG.md`.

## Using it
* **+ Running apps** - pick a process from the list (icon, process count, TCP/UDP sockets,
  interfaces in use, remote endpoints). **+ Browse...** or drag & drop an `.exe` / shortcut.
* **Configure** opens the interface view. Changes apply immediately; **Revert all changes** undoes
  everything since the screen was opened, **Done** closes it. Drag interfaces between **Allowed** and **Denied**.
  Order inside Allowed is the priority: traffic uses the first interface that is up and
  fails over to the next one. Denied interfaces are never used. *Deny all* = the app has no
  network. *Allow loopback* can block connections to services on this PC.
* The checkbox in the list turns a rule on/off. Changes apply to **already running** apps
  within ~2 seconds (the hook re-reads the config).
* **Launch** starts the app with the rule applied from the very first packet. Rules also
  attach to matching processes started elsewhere while Rerout runs (best effort).
* **Connections** shows every socket of the app and whether its interface is allowed.
  **Close** resets a TCP connection (or all not-allowed ones) so the app reconnects through the current
  rule. IPv4 TCP only and it needs administrator rights (the window offers "Restart as administrator").
* Closing the window minimizes to the tray. Tray menu: Open, Start with Windows, Quit.

## How it works
A DLL is injected into the target process and hooks `connect` / `WSAConnect` / `ConnectEx` /
`sendto` / `WSASendTo`. Right before a connect or send the socket gets
`IP_UNICAST_IF` / `IPV6_UNICAST_IF`, or the call fails when no allowed interface is up.
Loopback destinations are never bound to an interface; listening sockets are not touched.
`CreateProcessW` is hooked so child processes follow the same rule.

## Limitations
* DNS is resolved by a Windows service, the rule does not affect it.
* UDP sent through `WSASendMsg` is not intercepted yet.
* Applications that need administrator rights require Rerout to run as administrator.
* Protected processes (anti-cheat) block injection. Chromium browsers run their network code in a
  sandboxed process that cannot load the DLL; Rerout shows a hint with a launch flag that moves it
  into the main process (`--enable-features=NetworkServiceInProcess`, documented by Chromium; newer
  versions may ignore it).
* Windows 10/11 x64.
