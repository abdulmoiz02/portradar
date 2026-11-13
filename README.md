# portwatch

An interactive terminal dashboard for monitoring and managing the network ports on your Linux machine.

Type `portwatch` and it opens a full-screen dashboard (like `htop`, but for ports). Quit with `q` and you are back in your shell.

```
╭ portwatch ─────────────────────────────── ⚠ 17 hidden: needs sudo  ⟳ 1s ╮
│ 1 Listening (20) │ 2 Connections (37) │ 3 Events (2)                         │
╰──────────────────────────────────────────────────────────────────────────────╯
╭ Listening · sort: port ▲ ────────────────────────────────────────────────────╮
│  PROTO  PORT   ADDRESS       PID      PROCESS        USER                    │
│▶ tcp    3000   127.0.0.1     4821     node           alice                   │
│  tcp    5432   127.0.0.1     1302     postgres       postgres                │
╰──────────────────────────────────────────────────────────────────────────────╯
```

## Install and run

Requires Rust (stable) and Linux.

```bash
# run straight from the project folder
cargo run --release

# or install the `portwatch` command (goes to ~/.cargo/bin)
cargo install --path .
portwatch
```

If `portwatch` is "command not found" after installing, `~/.cargo/bin` is not on your `PATH`. Add this line to `~/.bashrc` and open a new terminal:

```bash
. "$HOME/.cargo/env"
```

### Update

After pulling or editing the code, reinstall over the old version:

```bash
cargo install --path . --force
```

### Commands

| Command | What it does |
|---|---|
| `portwatch` | Opens the dashboard |
| `portwatch list` | Prints listening ports as a table and exits (works in scripts and pipes) |
| `portwatch list --all` | Also includes every connection, with its state |
| `portwatch --help` / `--version` | Help and version |

## Using the dashboard

| Key | Action |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `g` `G` | Move the selection |
| `←` `→` / `Tab` / `1` `2` `3` | Switch tab: Listening, Connections, Events |
| `/` | Filter by port, process, user, address, PID (several words must all match) |
| `Esc` | Clear the filter |
| `s` / `r` | Change sort column (port, process, PID, user) / reverse the order |
| `k` | Stop the selected process (SIGTERM). Asks to confirm first |
| `K` | Force stop (SIGKILL). Asks to confirm first |
| `p` | Pause or resume refreshing |
| `?` | Help |
| `q` / `Ctrl+C` | Quit |

- **Green** rows opened in the last few seconds. **Red, struck-through** rows just closed.
- The **Details** panel shows the full command line, how long it has been running, its working directory and its open connections.
- The **Events** tab logs every listening port that opens or closes while portwatch is running.

### Stopping processes safely

- You always confirm first, and the popup shows the process name, PID and port.
- It refuses to kill PID 1 or portwatch itself.
- Just before sending the signal it checks that the PID still belongs to the same program (PIDs get reused), so it cannot hit the wrong process.
- If the process is a **Docker container** or a **systemd service**, the popup and Details panel warn you that killing it may just restart it, and tell you the right command (`docker stop ...` or `sudo systemctl stop ...`).
- Killing only works from the Listening tab.

### Permissions

As a normal user, the Linux kernel only lets you see which process owns a port if it is **your own** process. Other ports still appear, but with `-` as the process (greyed out), and the header shows a warning. To see and manage everything:

```bash
sudo "$(which portwatch)"
```

(Plain `sudo portwatch` usually fails because sudo does not search `~/.cargo/bin`.)

## How it works

1. **Scan** (`src/scanner.rs`): every second, read the kernel's socket tables `/proc/net/tcp`, `tcp6`, `udp`, `udp6`. Each socket has an inode number. Then walk `/proc/<pid>/fd/*` for every process you may inspect, find the socket file descriptors, and match their inodes to get the owning PID and name. No `ss`, `lsof` or root is required.
2. **Track** (`src/model.rs`): compare each scan with the previous one. New sockets are marked "new", missing ones are kept for 5 seconds as "closed", and listening-port changes are written to the event log. The selected row is remembered by socket identity, so the cursor does not jump when rows come and go.
3. **Inspect** (`src/inspect.rs`): for the selected row, read the command line, start time and `/proc/<pid>/cgroup` to detect Docker and systemd.
4. **Draw** (`src/ui.rs`, `src/app.rs`): a loop checks the keyboard every 100 ms (so keys feel instant), rescans every second, and redraws with [ratatui](https://ratatui.rs).
5. **Stop** (`src/kill.rs`): sends SIGTERM or SIGKILL after the safety checks above.

## Development

```bash
cargo test                                            # unit tests
cargo test print_sample_screen -- --ignored --nocapture   # preview the layout as text
cargo run                                             # run the dashboard
```

The dashboard needs a real terminal. Without one (for example in a pipe), `portwatch` tells you to use `portwatch list`.

## Known limits

- Linux only (it reads `/proc`).
- Other users' processes are hidden unless you run as root.
- Only TCP and UDP sockets are shown (not Unix sockets).
- No firewall management yet.
