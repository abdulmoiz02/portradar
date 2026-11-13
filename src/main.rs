// Milestone 1: scanner only. Several fields are not used until the dashboard
// exists, so silence dead-code warnings for now.
#[allow(dead_code)]
mod scanner;

fn main() {
    let entries = match scanner::scan() {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("portwatch: could not read /proc: {err}");
            std::process::exit(1);
        }
    };

    let listening: Vec<_> = entries.iter().filter(|e| e.listening).collect();

    println!(
        "{:<5} {:>5}  {:<39} {:>7}  {:<16} {}",
        "PROTO", "PORT", "ADDRESS", "PID", "PROCESS", "USER"
    );
    for e in &listening {
        let (pid, name) = match &e.process {
            Some(p) => (p.pid.to_string(), p.name.as_str()),
            None => ("-".to_string(), "-"),
        };
        println!(
            "{:<5} {:>5}  {:<39} {:>7}  {:<16} {}",
            e.protocol,
            e.local.port(),
            e.local.ip(),
            pid,
            name,
            e.user
        );
    }

    let hidden = listening.iter().filter(|e| e.process.is_none()).count();
    println!("\n{} listening", listening.len());
    if hidden > 0 {
        println!("{hidden} without process info (owned by other users; run with sudo to see them)");
    }
}
