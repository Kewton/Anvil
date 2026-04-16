use std::io::{self, Write};

use crossterm::{
    cursor, execute,
    terminal::{self, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::agent::{Agent, loop_run::AgentEvent};

pub fn run(agent: &mut Agent) -> Result<(), String> {
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0)
    )
    .map_err(|err| format!("failed to enter TUI: {err}"))?;

    let result = run_loop(agent);
    let _ = execute!(stdout, LeaveAlternateScreen);
    result
}

fn run_loop(agent: &mut Agent) -> Result<(), String> {
    println!("anvil tui");
    println!("type /exit to quit");

    let mut line = String::new();
    loop {
        print!("tui> ");
        io::stdout()
            .flush()
            .map_err(|err| format!("failed to flush stdout: {err}"))?;
        line.clear();
        let bytes = io::stdin()
            .read_line(&mut line)
            .map_err(|err| format!("failed to read line: {err}"))?;
        if bytes == 0 {
            break;
        }
        match agent.process_line(line.trim(), true)? {
            AgentEvent::Continue(Some(message)) => println!("{message}"),
            AgentEvent::Continue(None) => {}
            AgentEvent::Exit => break,
        }
    }
    Ok(())
}
