//! Web delivery: the same game shell as the native build, rendered into the
//! browser by Ratzilla and shipped as a static site.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use murmur_core::data::GameData;
use murmur_shell::{Shell, ShellInput};
use ratzilla::event::{KeyCode, KeyEvent};
use ratzilla::ratatui::Terminal;
use ratzilla::{DomBackend, WebRenderer};

fn main() -> io::Result<()> {
    console_error_panic_hook::set_once();

    let data = GameData::embedded().expect("embedded game data must be valid");
    let seed = js_sys::Date::now() as u64;
    let shell = Rc::new(RefCell::new(Shell::new(data, seed)));

    let backend = DomBackend::new()?;
    let terminal = Terminal::new(backend)?;

    let input_shell = Rc::clone(&shell);
    terminal.on_key_event(move |event| {
        if let Some(input) = translate(&event) {
            input_shell.borrow_mut().handle_input(input);
        }
    });

    terminal.draw_web(move |frame| {
        let mut shell = shell.borrow_mut();
        shell.tick();
        shell.draw(frame);
    });

    Ok(())
}

fn translate(event: &KeyEvent) -> Option<ShellInput> {
    match event.code {
        KeyCode::Up => Some(ShellInput::Up),
        KeyCode::Down => Some(ShellInput::Down),
        KeyCode::Left => Some(ShellInput::Left),
        KeyCode::Right => Some(ShellInput::Right),
        KeyCode::Enter => Some(ShellInput::Enter),
        KeyCode::Esc => Some(ShellInput::Esc),
        KeyCode::Backspace => Some(ShellInput::Backspace),
        KeyCode::Char(c) => Some(ShellInput::Char(c)),
        _ => None,
    }
}
