//! Web delivery: the same game shell as the native build, rendered into the
//! browser by Ratzilla and shipped as a static site.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use murmur_core::data::GameData;
use murmur_shell::{Shell, ShellInput};
use ratzilla::event::{MouseButton, MouseEventKind};
use ratzilla::ratatui::Terminal;
use ratzilla::{DomBackend, WebRenderer};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

/// On wasm a `main` that returns `Err` exits silently, so failures are
/// promoted to panics for console_error_panic_hook to report.
fn main() {
    console_error_panic_hook::set_once();
    if let Err(err) = run() {
        panic!("murmur-web failed to start: {err}");
    }
}

/// The single-slot campaign save in the browser's localStorage.
struct LocalStorageStore;

const SAVE_KEY: &str = "murmur-campaign";

impl murmur_campaign::CampaignStore for LocalStorageStore {
    fn load(&self) -> Option<String> {
        web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(SAVE_KEY)
            .ok()?
    }

    fn save(&mut self, document: &str) {
        if let Some(Ok(Some(storage))) = web_sys::window().map(|w| w.local_storage()) {
            let _ = storage.set_item(SAVE_KEY, document);
        }
    }

    fn clear(&mut self) {
        if let Some(Ok(Some(storage))) = web_sys::window().map(|w| w.local_storage()) {
            let _ = storage.remove_item(SAVE_KEY);
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let data = GameData::embedded().expect("embedded game data must be valid");
    let seed = js_sys::Date::now() as u64;
    let shell = Rc::new(RefCell::new(Shell::new(
        data,
        seed,
        Box::new(LocalStorageStore),
    )));

    let backend = DomBackend::new()?;
    let mut terminal = Terminal::new(backend)?;

    // Mouse: hover inspects, click activates (Ratzilla translates pixel
    // coordinates to terminal grid cells for us).
    let mouse_shell = Rc::clone(&shell);
    terminal.on_mouse_event(move |event| {
        let input = match event.kind {
            MouseEventKind::Moved => Some(ShellInput::MouseMove {
                column: event.col,
                row: event.row,
            }),
            MouseEventKind::SingleClick(MouseButton::Left)
            | MouseEventKind::DoubleClick(MouseButton::Left) => Some(ShellInput::MouseClick {
                column: event.col,
                row: event.row,
            }),
            _ => None,
        };
        if let Some(input) = input {
            mouse_shell.borrow_mut().handle_input(input);
        }
    })?;

    // Keyboard input attaches at the document level rather than through
    // Ratzilla's grid-focused listener, so the game responds immediately
    // without the player having to click the page first.
    let input_shell = Rc::clone(&shell);
    let on_key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if let Some(input) = translate(&event.key()) {
                event.prevent_default();
                input_shell.borrow_mut().handle_input(input);
            }
        });
    web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?
        .add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref())
        .map_err(|_| "failed to attach key listener")?;
    on_key.forget();

    terminal.draw_web(move |frame| {
        let mut shell = shell.borrow_mut();
        shell.tick();
        shell.draw(frame);
    });

    Ok(())
}

fn translate(key: &str) -> Option<ShellInput> {
    match key {
        "ArrowUp" => Some(ShellInput::Up),
        "ArrowDown" => Some(ShellInput::Down),
        "ArrowLeft" => Some(ShellInput::Left),
        "ArrowRight" => Some(ShellInput::Right),
        "Enter" => Some(ShellInput::Enter),
        "Escape" => Some(ShellInput::Esc),
        "Backspace" => Some(ShellInput::Backspace),
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(ShellInput::Char(c)),
                _ => None,
            }
        }
    }
}
