# portwatch

An interactive terminal dashboard for monitoring and managing the network ports on your Linux machine.

Run `portwatch` to open the dashboard: see which ports are listening, which process owns each one, watch changes live, and kill a process to free a port.

> Status: early development. Nothing is usable yet.

## Planned features

- Live table of listening ports (protocol, address, PID, process, user)
- Details panel for the selected port
- Highlighting of ports that just opened or closed
- Filter, sort and pause
- Kill a process from the dashboard, with confirmation
- Connections and Events tabs

## Development

Requires Rust (stable).

```bash
cargo run
```
