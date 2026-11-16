# portwatch

An interactive terminal dashboard for monitoring and managing the network ports on your Linux machine.

Type `portwatch` and it opens a full-screen dashboard (like `htop`, but for ports). Quit with `q` and you are back in your shell.

```
 ◉ portwatch   ports on this computer, live                          ● live · 1s
╭ Open ports ──────╮╭ Exposed ─────────╮╭ Connections ─────╮╭ Hidden ──────────╮
│        19        ││        7         ││        69        ││        17        │
│   listening now  ││ open to network  ││    active now    ││ need sudo to see │
╰──────────────────╯╰──────────────────╯╰──────────────────╯╰──────────────────╯
 1 Listening (19)   2 Connections (69)   3 Events (0)
╭ Listening ports · sorted by port ▲ ──────────────────────────────────────────╮
│  PORT       SERVICE         ACCESS           PROCESS          USER       UP  │
│▌ 3000/tcp   Dev server      ● this PC only   node 4821        alice      2h  │
│  5432/tcp   PostgreSQL      ● this PC only   postgres 1302    postgres   5d  │
│  80/tcp     Web (HTTP)      ▲ whole network  nginx 977        root       9d  │
╰──────────────────────────────────────────────────────────────────────────────╯
╭ node on port 3000 · 127.0.0.1 ───────────────────────────────────────────────╮
│ node (PID 4821) is listening on port 3000. Only this computer can reach it.  │
│ Port 3000 is usually Dev server · started 2h 5m ago by alice · 3 open conn…  │
│ $ node server.js                                                             │
╰──────────────────────────────────────────────────────────────────────────────╯
 ↑↓  Move   /  Search   k  Stop   ?  Help   q  Quit
```

The layout adapts to your terminal: on short windows the summary cards and then the details panel are hidden, and the footer drops its least important keys.

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
| `portwatch list --all` | Also includes every connection, with its state (one line per socket, for scripts) |
| `portwatch free <port>` | Finds what is using the port, asks, stops it, and checks that the port is really free (`--yes` skips the question, `--force` sends SIGKILL) |
| `portwatch inspect <port>` | Everything about the program using a port: process details, every socket it has, and where it comes from (systemd or Docker). Add `--json` for JSON you can pipe anywhere |
| `portwatch config` | Shows where the optional settings file goes and what it can contain |
| `portwatch --help` / `--version` | Help and version |

`portwatch free 3000` fixes the classic "address already in use" error. Unlike `fuser -k`, it asks first, tries SIGTERM before anything harsher, refuses to touch PID 1 or itself, and tells you whether the port was actually freed (exit code 0) or who still holds it (exit code 1).

## Using the dashboard

The first time you start it, a three-page tour explains what a port is and how to read a row. Press `?` for help at any time, or `:` for a command menu where you can find any action by typing its name.

| Key | Action |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `g` `G` | Move the selection |
| `←` `→` / `Tab` / `1` `2` `3` `4` | Switch tab: Listening, Connections, Events, Radar |
| `/` | Search by port, service, program, user, address, PID (several words must all match). Try `mysql`, `network` or `this pc` |
| `Esc` | Clear the search |
| `e` / `m` / `h` | Toggle: only ports open to the network / only my programs / hide system noise (DNS, printing, discovery) |
| `s` / `r` | Change sort column (port, process, PID, user, uptime, access) / reverse the order |
| `Enter` | Open the **Inspector**: full technical detail on the selected program (see below) |
| `t` | Switch the table between the simple view and the technical view |
| `k` / `K` | Stop the selected program (SIGTERM) / force stop (SIGKILL). Asks to confirm first |
| `f` | Free the port: stop its program, then check that the port really became free |
| `o` | Open the port in your web browser (TCP only) |
| `:` or `Ctrl+P` | Command menu |
| `p` | Pause or resume refreshing |
| `T` | Try the next colour theme (lasts until you quit; see Settings to keep one) |
| `?` | Help |
| `q` / `Ctrl+C` | Quit |

The mouse works too: click a tab or a row, scroll with the wheel. (While the mouse is captured, hold `Shift` to select text with the terminal. Turn it off with `mouse = false` in the settings file.)

- **Summary cards** at the top: open ports, how many are exposed to the network, active connections, and how many ports have an owner you cannot see.
- **One row per program per port.** If a server listens on both IPv4 and IPv6 (say `127.0.0.1` and `::1`), it is one row, and its ACCESS shows the most exposed address. Programs you can see are never merged with each other (different PIDs stay separate). Hidden owners are grouped by account and port. `portwatch list` still shows every socket.
- **The mark in the first column** is a quick opinion, with the reason in the Details panel:
  - `✓` looks normal (dim)
  - `★` a development server you started
  - `⚠` worth a look, for example a database open to the network, or an open port whose program you cannot see

  These are simple rules, not a security audit. They will sometimes be wrong, which is why the reason is shown.
- **SERVICE** is what that port number is normally used for (3306 is usually MySQL, 6379 Redis, and so on). It is a guess from the number only: any program can listen on any port.
- **ACCESS** tells you who can connect:
  - `● this PC only` (green): the port is bound to loopback, so only programs on this computer can reach it.
  - `▲ whole network` (yellow): bound to every network interface, so other devices on your network can try to connect.
  - `◆ one network` (yellow): bound to one specific network address.
- **UP** is how long the program has been running.
- **Green** rows opened in the last few seconds. **Red, struck-through** rows just closed.
- The **Details** panel explains the selected port in plain sentences, then shows how long it has been running, its open connections, the full command and its folder.
- The **Events** tab logs every listening port that opens or closes while portwatch is running.

### The Inspector (technical detail on request)

Press `Enter` on a row to open a full-screen Inspector for its program. `Esc` goes back. Switch tabs with `←` `→` or `1`-`6`, scroll with `↑` `↓` `PgUp` `PgDn` `Home` `End` or the wheel.

| Tab | Shows |
|---|---|
| **1 Overview** | A plain-English summary, the mark and its reason, and where to look next |
| **2 Process** | PID, parent and the chain of parents up to init, state, user/uid/gid, threads, memory, CPU time, start time, open files, folder, program file, and the command line one argument per line |
| **3 Sockets** | Every socket the program has: state, local and remote address, receive and send queue sizes, inode. Press `r` to look up host names for the remote addresses (see below) |
| **4 Origin** | The systemd service or Docker container it belongs to, and the raw cgroup paths |
| **5 Env** | Its environment variables. **Values are hidden** until you press `v`, and hidden again as soon as you change tab or close the Inspector |
| **6 Raw** | The JSON that `y` copies, then the exact `/proc` text everything above was built from, so you can check the work |

- `y` copies the JSON. It tries `wl-copy`, `xclip` and `xsel` if installed. If none is, it asks your terminal to copy (many terminals ignore that) and says so; it never claims a copy worked unless a program accepted it. Installing `wl-clipboard` (`sudo apt install wl-clipboard`) is the dependable fix on Wayland. `portwatch inspect <port> --json | wl-copy` works too.
- **Secrets:** environment variables are never part of the JSON, the Raw tab or the plain-text report. The JSON does include the program's **command line**, and command lines sometimes contain passwords, so look before you share it.
- **Host names:** `r` sends the remote addresses to your DNS resolver (reverse lookup). It only happens when you ask, loopback addresses are skipped, at most 50 are looked up, and results (including failures) are remembered.
- `k`, `f` and `o` work from the Inspector too: they close it first and then use the normal confirmation popup and safety checks.
- The Inspector refreshes with every scan. If the program exits while you are looking, its last known details stay on screen with a notice, and the actions are switched off.
- Details of other users' programs are not readable without `sudo`; the Inspector says so instead of guessing.

The **technical table** (`t`) swaps the friendly columns for the raw facts: the exact bind addresses, kernel socket state, PID and inode (a `+2` means two more sockets are merged into the row).

### The fun parts

- **Radar** (`4`): your listening ports as dots on three rings. The centre is this computer; the inner ring is ports only this computer can reach, the middle ring one network, the outer ring the whole network, so the more open a port is, the further out it sits. A sweep goes round every six seconds and flashes the dots it passes, and a port that just opened sends out a ripple. Each dot keeps its place unless another port in the same ring needs the same spot (positions are worked out from the port and program, never from the sort order), the selected port is always labelled, and `↑` `↓`, the filters, `k`, `f`, `o` and `Enter` work as on the Listening tab (sorting and the technical table only apply to the table). If the rings are too full for every dot, `+N not shown` says so.
- **History graphs** on the summary cards: the last 60 refreshes of each count (a minute at the default rate), squeezed to fit the card by keeping the highest value of each group, so a short spike still shows.
- **Confetti** when you free a port with `f` and portwatch has checked that the port really is free. Plain `k` never celebrates.
- **Did you know?** A line under the table with a fact about the selected port (SSH, DNS, MySQL, Redis and so on, kept to default ports and what the protocol does), or a tip about portwatch that changes every 15 seconds.
- **Themes:** `default`, `light`, `mono`, `nord`, `dracula`, `cyberpunk`, `matrix` and `colorblind` (blue and orange instead of green and red). `T` cycles through them. They use 256-colour values, so they work on any 256-colour terminal.

For a perfectly still screen set `animations = false` (no sweep, ripples or confetti; the radar stays put). `tips = false` hides the Did you know? line.

### Stopping processes safely

- You always confirm first, and the popup shows the process name, PID and port.
- It refuses to kill PID 1 or portwatch itself.
- Just before sending the signal it checks that the PID still belongs to the same program (PIDs get reused), so it cannot hit the wrong process.
- If the process is a **Docker container** or a **systemd service**, the popup and Details panel warn you that killing it may just restart it, and tell you the right command (`docker stop ...` or `sudo systemctl stop ...`).
- Killing only works from the Listening tab.
- `f` and `portwatch free` do not take "I sent the signal" for success: they scan again and say whether the port is free or who still holds it. Servers that run several worker processes may need `K`.
- `o` never opens a browser when portwatch runs as root.

### Settings (optional)

portwatch works without any settings. To change them, create `~/.config/portwatch/config.toml` (run `portwatch config` to see the exact path and a template):

```toml
refresh_seconds = 1     # how often the table refreshes (1 to 30)
theme = "default"       # default, light, mono, nord, dracula, cyberpunk, matrix, colorblind
ascii = false           # true = plain ASCII symbols and borders
mouse = true            # click tabs and rows, scroll with the wheel
animations = true       # false = no radar sweep, ripples or confetti
tips = true             # false = hide the "Did you know?" line
```

portwatch only ever reads this file. If it has a mistake, portwatch shows a message in the footer and uses the defaults. The only file portwatch writes is a tiny marker (`~/.local/state/portwatch/tour-seen`) so the tour is shown once, and never when run as root.

### Permissions

As a normal user, the Linux kernel only lets you see which process owns a port if it is **your own** process. Other ports still appear, but the process shows as `? needs sudo`, and the **Hidden** card counts them. To see and manage everything:

```bash
sudo "$(which portwatch)"
```

(Plain `sudo portwatch` usually fails because sudo does not search `~/.cargo/bin`.)

## How it works

1. **Scan** (`src/scanner.rs`): every second, read the kernel's socket tables `/proc/net/tcp`, `tcp6`, `udp`, `udp6`. Each socket has an inode number. Then walk `/proc/<pid>/fd/*` for every process you may inspect, find the socket file descriptors, and match their inodes to get the owning PID and name. No `ss`, `lsof` or root is required.
2. **Track** (`src/model.rs`): compare each scan with the previous one. New sockets are marked "new", missing ones are kept for 5 seconds as "closed", and listening-port changes are written to the event log. The selected row is remembered by socket identity, so the cursor does not jump when rows come and go.
3. **Inspect** (`src/inspect.rs`, `src/inspector.rs`): for the selected row, read the command line, start time and `/proc/<pid>/cgroup` to detect Docker and systemd. The Inspector collects everything in `src/inspector.rs` (process facts, sockets with queue sizes, origin, environment kept apart, raw `/proc` text) and `src/clip.rs` handles copying.
4. **Draw** (`src/ui.rs`, `src/app.rs`): a loop checks the keyboard every 100 ms (so keys feel instant), rescans every second, and redraws with [ratatui](https://ratatui.rs).
5. **Stop** (`src/kill.rs`): sends SIGTERM or SIGKILL after the safety checks above. `src/free.rs` adds the "free this port" check.
6. **Fun layer:** `src/radar.rs` lays out the radar as a pure function (same ports, same cells; time is passed in), `src/confetti.rs` and `src/spark.rs` are pure too, `src/tips.rs` holds the facts.
7. **Friendly layer:** `src/model.rs` merges rows for display only (events and highlighting stay per socket), `src/verdict.rs` holds the marks and the "system noise" list, `src/services.rs` the port names and exposure rules, `src/palette.rs` the command menu, `src/theme.rs` and `src/config.rs` the look and the optional settings.

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
